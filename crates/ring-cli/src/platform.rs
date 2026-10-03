use crate::store::io_error;
use ring_client::Result;
use std::{fs, path::Path};

#[cfg(any(windows, test))]
fn windows_quoted_arg(argument: &[u16]) -> std::io::Result<Vec<u16>> {
    if argument.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "NUL in daemon argument",
        ));
    }
    let mut quoted = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in argument {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        let count = if unit == b'"' as u16 {
            slashes * 2 + 1
        } else {
            slashes
        };
        quoted.extend(std::iter::repeat_n(b'\\' as u16, count));
        quoted.push(unit);
        slashes = 0;
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    quoted.push(b'"' as u16);
    Ok(quoted)
}

#[cfg(windows)]
pub fn spawn_daemon<'a>(
    program: &'a std::ffi::OsStr,
    args: impl Iterator<Item = &'a std::ffi::OsStr>,
    log: &fs::File,
) -> Result<()> {
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS},
        System::Threading::*,
    };

    let error = || io_error(std::io::Error::last_os_error(), "daemon start");
    let inherit = |file: &fs::File| -> Result<OwnedHandle> {
        let mut handle = std::ptr::null_mut();
        // Own inheritable duplicates; never change flags on the caller's handles.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                file.as_raw_handle(),
                GetCurrentProcess(),
                &mut handle,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    };
    let nul = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\NUL")
        .map_err(|e| io_error(e, "daemon start"))?;
    let nul = inherit(&nul)?;
    let stderr = inherit(log)?;
    let handles = [nul.as_raw_handle(), stderr.as_raw_handle()];
    let mut command_line = Vec::new();
    for arg in std::iter::once(program).chain(args) {
        if !command_line.is_empty() {
            command_line.push(b' ' as u16);
        }
        command_line.extend(
            windows_quoted_arg(&arg.encode_wide().collect::<Vec<_>>())
                .map_err(|e| io_error(e, "daemon arguments"))?,
        );
    }
    command_line.push(0);
    let executable = program.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let mut bytes = 0;
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(error());
    }
    // Pointer-aligned backing storage must outlive both the attribute list and CreateProcessW.
    let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let attributes = storage.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut bytes) } == 0 {
        return Err(error());
    }
    let result = (|| {
        if unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(&handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(error());
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = nul.as_raw_handle();
        startup.StartupInfo.hStdOutput = nul.as_raw_handle();
        startup.StartupInfo.hStdError = stderr.as_raw_handle();
        startup.lpAttributeList = attributes;
        let mut child = PROCESS_INFORMATION::default();
        // Keep only NUL and the private daemon log. Null environment/cwd preserve the
        // caller's SILICON_HOME, selected context, credentials and current directory.
        if unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | EXTENDED_STARTUPINFO_PRESENT,
                std::ptr::null(),
                std::ptr::null(),
                &startup.StartupInfo,
                &mut child,
            )
        } == 0
        {
            return Err(error());
        }
        unsafe {
            drop(OwnedHandle::from_raw_handle(child.hThread));
            drop(OwnedHandle::from_raw_handle(child.hProcess));
        }
        Ok(())
    })();
    unsafe {
        DeleteProcThreadAttributeList(attributes);
    }
    result
}

#[cfg(unix)]
pub use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
pub trait OpenOptionsExt {
    fn mode(&mut self, _mode: u32) -> &mut Self;
}
#[cfg(windows)]
impl OpenOptionsExt for fs::OpenOptions {
    fn mode(&mut self, _mode: u32) -> &mut Self {
        self
    }
}

pub fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|e| io_error(e, "protect directory"))?;
    }
    #[cfg(windows)]
    {
        // Use .NET ACL APIs so an inherited PowerShell 7 module path cannot break Windows PowerShell.
        // Change only the DACL; replacing the owner can require privileges that normal users lack.
        let script = r#"$ErrorActionPreference='Stop';$p=$env:RING_LOCAL_PATH;$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$acl=[System.IO.Directory]::GetAccessControl($p);$acl.SetSecurityDescriptorSddlForm(('D:P(A;OICI;FA;;;'+$sid+')'),[System.Security.AccessControl.AccessControlSections]::Access);[System.IO.Directory]::SetAccessControl($p,$acl)"#;
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
            .env_remove("PSModulePath")
            .output()
            .map_err(|e| io_error(e, "protect directory"))?;
        if !result.status.success() {
            return Err(io_error(
                format!(
                    "Could not apply owner-only Windows storage ACL: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ),
                "protect directory",
            ));
        }
    }
    Ok(())
}
pub fn private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| io_error(e, "protect file"))?;
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}
pub fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| io_error(e, "executable permissions"))?;
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}
pub fn protected_secret(path: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)
            .map_err(|e| io_error(e, "test authentication"))?
            .permissions()
            .mode()
            & 0o077
            == 0)
    }
    #[cfg(windows)]
    {
        let script="$ErrorActionPreference='Stop';$p=$env:RING_LOCAL_PATH;$acl=if([System.IO.Directory]::Exists($p)){[System.IO.Directory]::GetAccessControl($p)}else{[System.IO.File]::GetAccessControl($p)};$allowed=@([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value,'S-1-5-18','S-1-5-32-544');foreach($rule in $acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])){if($rule.AccessControlType -eq 'Allow' -and $allowed -notcontains $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value){exit 2}}";
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
            .env_remove("PSModulePath")
            .status()
            .map_err(|e| io_error(e, "test authentication"))?;
        Ok(status.success())
    }
}

#[cfg(test)]
mod argument_tests {
    use super::windows_quoted_arg;

    #[test]
    fn windows_arguments_preserve_quotes_slashes_empty_and_unicode() {
        for (input, expected) in [
            ("", r#""""#),
            ("--org", r#""--org""#),
            ("org with spaces", r#""org with spaces""#),
            ("组织", r#""组织""#),
            ("a\"b", r#""a\"b""#),
            (r"C:\ring\", r#""C:\ring\\""#),
            (r#"a\"b"#, r#""a\\\"b""#),
        ] {
            let wide = input.encode_utf16().collect::<Vec<_>>();
            assert_eq!(
                windows_quoted_arg(&wide).unwrap(),
                expected.encode_utf16().collect::<Vec<_>>()
            );
        }
        assert_eq!(windows_quoted_arg(&[0xd800]).unwrap(), [34, 0xd800, 34]);
        assert!(windows_quoted_arg(&[0]).is_err());
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn current_user_can_create_private_storage_and_inherited_files() {
        let directory = std::env::temp_dir().join(format!("ring-private-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        private_dir(&directory).unwrap();
        let secret = directory.join("session.json");
        fs::write(&secret, b"test fixture").unwrap();
        assert!(protected_secret(&secret).unwrap());
        private_dir(&directory).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
