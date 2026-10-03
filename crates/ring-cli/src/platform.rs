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
        // New ACL contains only the current user's SID; children inherit this owner-only rule.
        let script="$p=$env:RING_LOCAL_PATH;$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;$acl=New-Object System.Security.AccessControl.DirectorySecurity;$acl.SetOwner($sid);$acl.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow');$acl.AddAccessRule($rule);Set-Acl -LiteralPath $p -AclObject $acl";
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
            .output()
            .map_err(|e| io_error(e, "protect directory"))?;
        if !result.status.success() {
            return Err(io_error(
                "Could not apply owner-only Windows storage ACL",
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
        let script="$acl=Get-Acl -LiteralPath $env:RING_LOCAL_PATH;$allowed=@([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value,'S-1-5-18','S-1-5-32-544');foreach($rule in $acl.Access){if($rule.AccessControlType -eq 'Allow' -and $allowed -notcontains $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value){exit 2}}";
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("RING_LOCAL_PATH", path)
            .status()
            .map_err(|e| io_error(e, "test authentication"))?;
        Ok(status.success())
    }
}
