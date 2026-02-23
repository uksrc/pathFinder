use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn mount_operation(fits_path: &str, sudo_group: &str, sudo_user: &str) -> Result<()> {
    let fits_path = Path::new(fits_path);
    let fits_file = fits_path.file_name()
        .context("Invalid FITS path")?
        .to_str()
        .context("Invalid UTF-8 in filename")?;

    let fits_dir = fits_path.parent().and_then(|p| p.to_str()).unwrap_or("");

    // Extract the bind name from the filename (remove extension)
    let bind_name = fits_file
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(fits_file);

    let home = PathBuf::from("/home").join(sudo_user);
    let bind_dir = home.join(".binds").join(bind_name);
    let projects_dir = home.join("projects");
    let projects_file = projects_dir.join(fits_file);
    let skadata_src = PathBuf::from("/skadata").join(sudo_group).join(fits_dir);

    // Check if already mounted
    if is_mountpoint(&bind_dir)? {
        anyhow::bail!(
            "Error: {} is already mounted.",
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
    let source_file = bind_dir.join(fits_file);
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
            fits_file,
            projects_file.display()
        );
    } else {
        anyhow::bail!(
            "Error: Mount verification failed for {} at {}",
            fits_file,
            projects_file.display()
        );
    }

    Ok(())
}

pub fn unmount_operation(fits_path: &str, sudo_user: &str) -> Result<()> {
    let fits_path = Path::new(fits_path);
    let fits_file = fits_path.file_name()
        .context("Invalid FITS path")?
        .to_str()
        .context("Invalid UTF-8 in filename")?;

    let bind_name = fits_file
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(fits_file);

    let home = PathBuf::from("/home").join(sudo_user);
    let bind_dir = home.join(".binds").join(bind_name);
    let projects_file = home.join("projects").join(fits_file);

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

    println!("Unmounted {} from {}", fits_file, projects_file.display());

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
