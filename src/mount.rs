use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Mounts a data file from the RSE storage to the user's home directory using bindfs.
///
/// Creates necessary directories and bind mounts to make the data file accessible to the user
/// with appropriate permissions. The file is mounted to `~/.binds/<filename>` and linked to
/// `~/projects/<namespace>/<filename>`.
///
/// # Parameters
///
/// * `data_path` - Full path to the data file on the RSE storage.
///   Example: `"/daac/08/06/2022-01-01_12-00-00.fits"`
///
/// * `sudo_group` - The namespace/group for the data
///   Example: `"daac"`
///
/// * `sudo_user` - The username of the user running the command (from SUDO_USER environment variable).
///   Example: `"jsmith"`
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if any step fails (directory creation, mounting, etc.).
///
/// # Example
///
/// ```no_run
/// mount_operation("/daac/08/06/2022-01-01_12-00-00.fits", "daac", "jsmith")?;
/// ```
pub fn mount_operation(data_path: &str, sudo_group: &str, sudo_user: &str) -> Result<()> {
    let data_path = Path::new(data_path);
    let data_file = data_path.file_name()
        .context("Invalid FITS path")?
        .to_str()
        .context("Invalid characters in filename that cannot be represented in UTF-8")?;

    let data_dir = data_path.parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .trim_start_matches('/');  // Strip leading slash for proper path joining

    // Extract the bind name from the filename (remove extension)
    let bind_name = data_file
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(data_file);

    let home = PathBuf::from("/home").join(sudo_user);
    let bind_dir = home.join(".binds").join(bind_name);
    let projects_dir = home.join("projects").join(sudo_group);
    let projects_file = projects_dir.join(data_file);
    // TODO: Read the SKA data base path (default: `/skadata`) from config or env variable instead of hardcoding - check it exists at startup
    let skadata_src = PathBuf::from("/skadata").join(data_dir);

    // Output debug information about paths being used
    println!("Data file: {}", data_file);
    println!("Bind name: {}", bind_name);
    println!("SKA data source path: {}", skadata_src.display());
    println!("Bind directory: {}", bind_dir.display());
    println!("Projects directory: {}", projects_dir.display());
    println!("Projects file: {}", projects_file.display());

    // TODO: Check if already mounted - if so, check that the file is also mounted to the projects directory; if both true: bail
    if is_mountpoint(&bind_dir)? {
        anyhow::bail!(
            "{} is already mounted.",
            bind_dir.display()
        );
    }

    // Create directories
    fs::create_dir_all(&bind_dir)
        .with_context(|| format!("Failed to create {}", bind_dir.display()))?;
    fs::create_dir_all(&projects_dir)
        .with_context(|| format!("Failed to create {}", projects_dir.display()))?;

    // Touch projects file
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&projects_file)
        .with_context(|| format!("Failed to create placeholder file {}", projects_file.display()))?;

    // Set ownership and permissions
    let user_group = format!("{}:{}", sudo_user, sudo_user);

    run_command(
        "chown",
        &["-R", &user_group, home.join(".binds").to_str().unwrap()],
        "Set ownership of .binds directory",
    )?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bind_dir)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&bind_dir, perms)?;
    }

    run_command(
        "chown",
        &["-R", &user_group, projects_dir.to_str().unwrap()],
        "Set ownership of projects directory",
    )?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&projects_file)?.permissions();
        perms.set_mode(0o500);
        fs::set_permissions(&projects_file, perms)?;
    }

    // Run bindfs
    run_command(
        "bindfs",
        &[
            "--perms=0700",
            &format!("--force-user={}", sudo_user),
            &format!("--force-group={}", sudo_user),
            skadata_src.to_str().unwrap(),
            bind_dir.to_str().unwrap(),
        ],
        "Mount with bindfs",
    )?;

    // Bind mount the file
    let source_file = bind_dir.join(data_file);
    run_command(
        "mount",
        &[
            "--bind",
            source_file.to_str().unwrap(),
            projects_file.to_str().unwrap(),
        ],
        "Bind mount file",
    )?;

    // Verify mount
    if is_mountpoint(&projects_file)? {
        println!(
            "Mount verification successful: {} is mounted at {}",
            data_file,
            projects_file.display()
        );
    } else {
        anyhow::bail!(
            "Error: Mount verification failed for {} at {}",
            data_file,
            projects_file.display()
        );
    }

    Ok(())
}

pub fn unmount_operation(data_path: &str, sudo_user: &str) -> Result<()> {
    let data_path = Path::new(data_path);
    let data_file = data_path.file_name()
        .context("Invalid FITS path")?
        .to_str()
        .context("Invalid UTF-8 in filename")?;

    let bind_name = data_file
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(data_file);

    let home = PathBuf::from("/home").join(sudo_user);
    let bind_dir = home.join(".binds").join(bind_name);
    let projects_file = home.join("projects").join(data_file);

    // Unmount (ignore errors if not mounted)
    let _ = run_command("umount", &[projects_file.to_str().unwrap()], "Unmount projects file");
    let _ = run_command("umount", &[bind_dir.to_str().unwrap()], "Unmount bind directory");

    // Remove directories/files
    if bind_dir.exists() {
        fs::remove_dir_all(&bind_dir)
            .with_context(|| format!("Failed to remove {}", bind_dir.display()))?;
    }

    if projects_file.exists() {
        fs::remove_file(&projects_file)
            .with_context(|| format!("Failed to remove {}", projects_file.display()))?;
    }

    println!("Unmounted {} from {}", data_file, projects_file.display());

    Ok(())
}

fn is_mountpoint(path: &Path) -> Result<bool> {
    let output = Command::new("mountpoint")
        .arg("-q")
        .arg(path)
        .output()
        .context("Failed to execute mountpoint command")?;

    Ok(output.status.success())
}

fn run_command(cmd: &str, args: &[&str], description: &str) -> Result<()> {

    println!("Running command: {} {}", cmd, args.join(" "));

    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute: {} {}", cmd, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{} failed: {}",
            description,
            stderr.trim()
        );
    }

    Ok(())
}
