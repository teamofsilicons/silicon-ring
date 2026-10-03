use crate::store::io_error;
use ring_client::Result;
use std::{fs, path::Path};

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
        // Change only the DACL; replacing the owner can require privileges that normal users lack.
        let script = r#"$ErrorActionPreference='Stop';$p=$env:RING_LOCAL_PATH;$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$acl=Get-Acl -LiteralPath $p;$acl.SetSecurityDescriptorSddlForm(('D:P(A;OICI;FA;;;'+$sid+')'),[System.Security.AccessControl.AccessControlSections]::Access);Set-Acl -LiteralPath $p -AclObject $acl"#;
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
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
        let script="$ErrorActionPreference='Stop';$acl=Get-Acl -LiteralPath $env:RING_LOCAL_PATH;$allowed=@([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value,'S-1-5-18','S-1-5-32-544');foreach($rule in $acl.Access){if($rule.AccessControlType -eq 'Allow' -and $allowed -notcontains $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value){exit 2}}";
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
            .status()
            .map_err(|e| io_error(e, "test authentication"))?;
        Ok(status.success())
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
