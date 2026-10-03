#!/usr/bin/env python3
"""Package a native CLI binary and sign compatible update metadata with Ed25519.
Signing requires OpenSSL 3 and RING_RELEASE_SIGNING_KEY (PEM or a 32-byte seed in hex).
No release is published by this script. Use --unsigned only for local/CI build artifacts.
"""
import argparse, base64, hashlib, json, os, pathlib, re, shutil, subprocess, tarfile, tempfile, zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGETS = {(platform, arch) for platform in ("macos", "linux", "windows") for arch in ("x86_64", "aarch64")}

def run(*args):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout

def sign(message, key):
    with tempfile.TemporaryDirectory(prefix="ring-release-sign-") as temp:
        temp = pathlib.Path(temp)
        private = temp / "private.pem"
        if re.fullmatch(r"[0-9a-fA-F]{64}", key.strip()):
            private.write_bytes(bytes.fromhex("302e020100300506032b657004220420" + key.strip()))
            form = "DER"
        elif "BEGIN PRIVATE KEY" in key:
            private.write_text(key)
            form = "PEM"
        else:
            raise ValueError("Signing key must be an Ed25519 PEM or a 32-byte hex seed")
        private.chmod(0o600)
        message_path = temp / "message"
        message_path.write_bytes(message)
        signature = run("openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(private), "-keyform", form, "-in", str(message_path))
        public = run("openssl", "pkey", "-in", str(private), "-inform", form, "-pubout", "-outform", "DER")
        if len(signature) != 64 or not public.startswith(bytes.fromhex("302a300506032b6570032100")):
            raise ValueError("The release signing key must be Ed25519")
        return base64.b64encode(signature).decode(), public[-32:].hex()

def package(args):
    if (args.platform, args.arch) not in TARGETS:
        raise ValueError("Unsupported release platform/architecture")
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", args.version):
        raise ValueError("Version must be a semantic version without a leading v")
    if not args.base_url.startswith("https://"):
        raise ValueError("Release base URL must use HTTPS")
    args.output.mkdir(parents=True, exist_ok=True)
    executable = "ring.exe" if args.platform == "windows" else "ring"
    stem = f"ring-{args.version}-{args.platform}-{args.arch}"
    binary_name = stem + (".exe" if args.platform == "windows" else "")
    binary = args.output / binary_name
    shutil.copyfile(args.binary, binary)
    binary.chmod(0o755)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    url = args.base_url.rstrip("/") + "/" + binary_name
    message = f"{args.version}\n1\n{args.platform}\n{args.arch}\n{digest}\n{url}".encode()
    signature, public_key = (None, None) if args.unsigned else sign(message, os.environ["RING_RELEASE_SIGNING_KEY"])
    metadata = {"version": args.version, "protocol_major": 1, "platform": args.platform, "arch": args.arch, "channel": "stable", "sha256": digest, "url": url, "signature": signature, "signing_public_key": public_key, "signed": not args.unsigned}
    files = [(binary, executable), (ROOT / "docs/cli-runtime.md", "docs/cli-runtime.md"), (ROOT / "udd/cli.md", "docs/cli.md"), (ROOT / "udd/api.md", "docs/api.md")]
    if args.platform == "windows":
        archive = args.output / (stem + ".zip")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for source, name in files:
                if source.exists(): output.write(source, name)
    else:
        archive = args.output / (stem + ".tar.gz")
        with tarfile.open(archive, "w:gz") as output:
            for source, name in files:
                if source.exists(): output.add(source, arcname=name, recursive=False)
    metadata["archive_url"] = args.base_url.rstrip("/") + "/" + archive.name
    metadata["archive_sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()
    (args.output / (stem + ".json")).write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps({"binary": str(binary), "archive": str(archive), "signed": not args.unsigned, "public_key": public_key}))
    return metadata

def honeycomb_package(args):
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", args.version):
        raise ValueError("Version must be a semantic version")
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ring-honeycomb-") as temp:
        stage=pathlib.Path(temp)
        manifest=["format_version: 1", "app_id: ring", f"version: {args.version}", "bin:", "  ring: main", "targets:"]
        for platform,arch in sorted(TARGETS):
            target=f"{platform}-{arch}"
            executable="ring.exe" if platform=="windows" else "ring"
            root=stage/"targets"/target
            root.mkdir(parents=True)
            source=args.binary/(f"ring-{target}"+(".exe" if platform=="windows" else ""))
            if not source.is_file() or source.is_symlink():
                raise ValueError(f"A regular native binary is required for {target}")
            shutil.copyfile(source,root/executable)
            (root/executable).chmod(0o755)
            for name,path in [("cli-runtime.md",ROOT/"docs/cli-runtime.md"),("cli.md",ROOT/"udd/cli.md"),("api.md",ROOT/"udd/api.md")]:
                if path.exists():
                    (root/"docs").mkdir(exist_ok=True)
                    shutil.copyfile(path,root/"docs"/name)
            setup="setup.ps1" if platform=="windows" else "setup.sh"
            (root/setup).write_text("$ErrorActionPreference = 'Stop'\nring daemon start\nif ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }\n" if platform=="windows" else "#!/bin/sh\nset -eu\nring daemon start\n")
            (root/setup).chmod(0o755)
            manifest += [f"  {target}:",f"    root: targets/{target}","    executables:",f"      main: {executable}",f"    install_script: {setup}"]
        (stage/"honeycomb.yaml").write_text("\n".join(manifest)+"\n")
        archive=args.output/f"ring-{args.version}-honeycomb.tar.gz"
        if shutil.which("honeycomb"):
            subprocess.run(["honeycomb","validate",str(stage)],check=True)
            subprocess.run(["honeycomb","pack",str(stage),"--output",str(archive.resolve())],check=True)
        else:
            with tarfile.open(archive,"w:gz") as output:
                for path in sorted(stage.rglob("*")):
                    output.add(path,arcname=path.relative_to(stage).as_posix(),recursive=False)
        print(json.dumps({"honeycomb_archive":str(archive),"targets":len(TARGETS)}))
        return archive

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--platform", choices=["macos", "linux", "windows"])
    parser.add_argument("--arch", choices=["x86_64", "aarch64"])
    parser.add_argument("--version", required=True)
    parser.add_argument("--base-url", default="https://example.invalid/unpublished")
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("dist"))
    parser.add_argument("--unsigned", action="store_true")
    parser.add_argument("--honeycomb", action="store_true", help="Build one six-target archive; --binary points to the collected native binaries directory")
    args=parser.parse_args()
    honeycomb_package(args) if args.honeycomb else package(args)
