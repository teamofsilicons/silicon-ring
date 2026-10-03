use crate::store::io_error;
use ring_client::Result;
use std::{fs, path::Path};

#[cfg(windows)]
pub fn prevent_stdio_inheritance() -> Result<()> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    // Command redirects a child's standard streams but still inherits every other inheritable
    // handle. Keep the original caller's pipes out of the long-lived daemon so its caller sees EOF.
    for which in [-10_i32, -11, -12] {
        unsafe {
            let handle = GetStdHandle(which as u32);
            if !handle.is_null()
                && handle != (-1_isize) as *mut c_void
                && SetHandleInformation(handle, 1, 0) == 0
            {
                return Err(io_error(std::io::Error::last_os_error(), "daemon handles"));
            }
        }
    }
    Ok(())
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
