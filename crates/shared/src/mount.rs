//! Mount and unmount operations for making RSE data accessible to users.
//!
//! Uses `bindfs` and `mount --bind` to remap filesystem permissions, exposing a file from the RSE storage
//! at `/skadata` into the user's home directory under `~/projects/<namespace>/`.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use users;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Returns `true` if `path` is already owned by `username` (uid and gid both match).
///
/// Falls back to `false` if the user cannot be resolved in the password database or
/// there was any error in obtaining or comparing file to user information.
fn dir_already_owned_by(path: &Path, username: &str) -> bool {
    #[cfg(unix)]
    {
        let (uid, gid) = match users::get_user_by_name(username) {
            Some(user) => (user.uid(), user.primary_group_id()),
            None => return false, // If we can't resolve the user, we can't confirm ownership, so assume it's not owned by them.
        };

        fs::metadata(path)
            .map(|m| m.uid() == uid && m.gid() == gid)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Abstraction over system commands, allowing real system calls in production and mock system calls during testing.
trait Runner {
    /// Executes an external command, returning an error if it exits non-zero.
    ///
    /// * `cmd` - The command to run (e.g. `"bindfs"`, `"mount"`, `"chown"`).
    /// * `args` - Arguments to pass to the command.
    /// * `description` - Human-readable label included in any error message.
    fn run_command(&self, cmd: &str, args: &[&str], description: &str) -> Result<()>;

    /// Returns `true` if `path` is an active mount point.
    fn is_mountpoint(&self, path: &Path) -> Result<bool>;
}

/// Production [`Runner`] that delegates to the real system commands.
struct SystemRunner;

impl Runner for SystemRunner {
    fn run_command(&self, cmd: &str, args: &[&str], description: &str) -> Result<()> {
        run_command(cmd, args, description)
    }
    fn is_mountpoint(&self, path: &Path) -> Result<bool> {
        is_mountpoint(path)
    }
}

/// Mounts a data file from the RSE storage to the user's home directory using bindfs.
///
/// Creates necessary directories and bind mounts to make the data file accessible to the user
/// with appropriate permissions. The file is mounted to `~/.binds/<namespace>/<filename>` and linked to
/// `~/projects/<namespace>/<filename>`.
///
/// # Parameters
///
/// * `data_path` - Full path to the data file on the RSE storage.
///   Example: `"/daac/08/06/2022-01-01_12-00-00.fits"`
///
/// * `namespace` - The namespace/group for the data
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
/// use pathfinder_shared::mount::mount_data_operation;
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     mount_data_operation(
///         "/daac/08/06/2022-01-01_12-00-00.fits",
///         "daac",
///         "jsmith",
///         "/home/jsmith",
///     )?;
///     Ok(())
/// }
/// ```
pub fn mount_data_operation(
    data_path: &str,
    namespace: &str,
    sudo_user: &str,
    base_path: &str,
) -> Result<()> {
    mount_data_operation_impl(
        data_path,
        namespace,
        sudo_user,
        base_path,
        Path::new("/skadata"),
        &SystemRunner,
    )
}

/// Internal implementation of the mount operation, parameterized over the base paths and command runner for testing.
fn mount_data_operation_impl(
    data_path: &str,
    namespace: &str,
    sudo_user: &str,
    base_path: &str,
    skadata_base: &Path,
    runner: &dyn Runner,
) -> Result<()> {
    if !skadata_base.exists() {
        anyhow::bail!(
            "The RSE mount point {} does not exist on this host. \
             Please ensure the RSE is mounted to the host before using pathFinder.",
            skadata_base.display()
        );
    }

    let data_path = Path::new(data_path);
    let data_file = data_path
        .file_name()
        .context("Invalid FITS path")?
        .to_str()
        .context("Invalid characters in filename that cannot be represented in UTF-8")?;

    let data_dir = data_path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .trim_start_matches('/'); // Strip leading slash for proper path joining

    // Extract the bind name from the filename (remove extension)
    let bind_name = data_file
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(data_file);

    let base_path = Path::new(base_path);
    let bind_dir = base_path.join(".binds").join(namespace).join(bind_name);
    let projects_dir = base_path.join("projects").join(namespace);
    let projects_file = projects_dir.join(data_file);
    let skadata_dir = skadata_base.join(data_dir);
    let skadata_file = skadata_dir.join(data_file);

    if !skadata_file.exists() {
        anyhow::bail!(
            "File '{}' not found at {}. The RSE may not be mounted at this site, \
             or the specific data may not have been staged here.",
            data_file,
            skadata_dir.display()
        );
    }

    // TODO: Check if already mounted - if so, check that the file is also mounted to the projects directory; if both true: bail
    if runner.is_mountpoint(&bind_dir)? {
        anyhow::bail!("{} is already mounted.", bind_dir.display());
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
        .with_context(|| {
            format!(
                "Failed to create placeholder file {}",
                projects_file.display()
            )
        })?;

    // Set ownership and permissions
    let user_group = format!("{}:{}", sudo_user, sudo_user);

    // Set ownership of .binds/<bind_name> directory.
    // We do NOT use recursive chown, as this would
    // fail if the directory happens to contain read-only bindfs content from a prior run.
    // Skip entirely when the directory is already correctly owned (e.g. a second invocation for a
    // different file that shares the same .binds/<bind_name> but has already been set up).
    if !dir_already_owned_by(&bind_dir, sudo_user) {
        runner.run_command(
            "chown",
            &[&user_group, bind_dir.to_str().unwrap()],
            "Set ownership of .binds directory",
        )?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&bind_dir)?.permissions();
        if perms.mode() & 0o777 != 0o600 {
            let mut new_perms = perms;
            new_perms.set_mode(0o600);
            fs::set_permissions(&bind_dir, new_perms)?;
        }
    }

    // Set ownership of projects/<namespace> directory.
    // We do NOT use recursive chown, as this would
    // fail if the directory happens to contain read-only bindfs content from a prior run.
    // Skip entirely when the directory is already correctly owned (e.g. a second invocation for a
    // different file that shares the same .binds/<bind_name> but has already been set up).
    if !dir_already_owned_by(&projects_dir, sudo_user) {
        runner.run_command(
            "chown",
            &[&user_group, projects_dir.to_str().unwrap()],
            "Set ownership of projects directory",
        )?;
    }

    // Set ownership of the placeholder file inside projects/<namespace>/ to ensure it's accessible to the target user.
    runner.run_command(
        "chown",
        &[&user_group, projects_file.to_str().unwrap()],
        "Set ownership of projects placeholder file",
    )?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&projects_file)?.permissions();
        if perms.mode() & 0o777 != 0o500 {
            let mut new_perms = perms;
            new_perms.set_mode(0o500);
            fs::set_permissions(&projects_file, new_perms)?;
        }
    }

    // Run bindfs
    runner.run_command(
        "bindfs",
        &[
            "--perms=0700",
            &format!("--force-user={}", sudo_user),
            &format!("--force-group={}", sudo_user),
            skadata_dir.to_str().unwrap(),
            bind_dir.to_str().unwrap(),
        ],
        "Mount with bindfs",
    )?;

    // Bind mount the file
    let source_file = bind_dir.join(data_file);
    runner.run_command(
        "mount",
        &[
            "--bind",
            source_file.to_str().unwrap(),
            projects_file.to_str().unwrap(),
        ],
        "Bind mount file",
    )?;

    // Verify mount
    if runner.is_mountpoint(&projects_file)? {
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

/// Unmounts a previously mounted data file and cleans up the associated directories.
///
/// Unmounts the bind-mounted file at `{base_path}/projects/<namespace>/<file_name>` and the
/// bindfs directory at `{base_path}/.binds/<namespace>/<stem>`, then removes both from the filesystem.
/// Unmount errors are ignored in case the paths are not currently mounted.
///
/// # Arguments
/// * `base_path` - The base directory containing the user's `.binds` and `projects` directories
///   (typically `/home/<user>`).
/// * `namespace` - The namespace the file belongs to (e.g. `"daac"`).
/// * `file_name` - The name of the file to unmount (e.g. `"random10MiB.bin"`).
pub fn unmount_operation(base_path: &str, namespace: &str, file_name: &str) -> Result<()> {
    unmount_operation_impl(Path::new(base_path), namespace, file_name)
}

/// Internal implementation of the unmount operation, parameterized over the base path for testing.
fn unmount_operation_impl(base_path: &Path, namespace: &str, file_name: &str) -> Result<()> {
    let bind_name = file_name
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(file_name);

    let bind_dir = base_path.join(".binds").join(namespace).join(bind_name);
    let projects_dir = base_path.join("projects").join(namespace);
    let projects_file = projects_dir.join(file_name);

    // Unmount (ignore errors if not mounted)
    let _ = run_command(
        "umount",
        &[projects_file.to_str().unwrap()],
        "Unmount projects file",
    );
    let _ = run_command(
        "umount",
        &[bind_dir.to_str().unwrap()],
        "Unmount bind directory",
    );

    // Remove directories/files
    if bind_dir.exists() {
        fs::remove_dir_all(&bind_dir)
            .with_context(|| format!("Failed to remove {}", bind_dir.display()))?;
    }

    if projects_file.exists() {
        fs::remove_file(&projects_file)
            .with_context(|| format!("Failed to remove {}", projects_file.display()))?;
    }

    println!("Unmounted {} from {}", file_name, projects_file.display());

    Ok(())
}

/// Returns `true` if the given path is a mount point, determined using the `mountpoint` command.
fn is_mountpoint(path: &Path) -> Result<bool> {
    let output = Command::new("mountpoint")
        .arg("-q")
        .arg(path)
        .output()
        .context("Failed to execute mountpoint command")?;

    Ok(output.status.success())
}

/// Helper function to run a system command and return an error if it fails, including the command's stderr in the error message.
fn run_command(cmd: &str, args: &[&str], description: &str) -> Result<()> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute: {} {}", cmd, args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{} failed: {}", description, stderr.trim());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use tempfile::TempDir;

    // Real-world data path as returned by the DM API locate endpoint.
    const DATA_PATH: &str = "/daac/08/06/random10MiB.bin";
    const DATA_FILE: &str = "random10MiB.bin";
    const NAMESPACE: &str = "daac";
    const USER: &str = "jsmith";

    // Populate skadata_base/<data_dir>/<data_file> so the existence check passes.
    fn seed_skadata(skadata: &Path) {
        let data_dir = skadata.join("daac/08/06");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("random10MiB.bin"), b"").unwrap();
    }

    /// Mock runner that accepts all commands as no-ops.
    ///
    /// `is_mountpoint` returns `false` on the first call (the "already mounted?" guard)
    /// and `true` on subsequent calls (the post-mount verification).
    struct MockRunner {
        mountpoint_calls: Cell<usize>,
    }

    impl MockRunner {
        fn new() -> Self {
            Self {
                mountpoint_calls: Cell::new(0),
            }
        }
    }

    impl Runner for MockRunner {
        fn run_command(&self, _cmd: &str, _args: &[&str], _description: &str) -> Result<()> {
            Ok(())
        }
        fn is_mountpoint(&self, _path: &Path) -> Result<bool> {
            let n = self.mountpoint_calls.get();
            self.mountpoint_calls.set(n + 1);
            Ok(n > 0)
        }
    }

    // --- mount: /skadata mount point ---

    #[test]
    fn mount_errors_when_skadata_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata"); // intentionally not created
        let home = tmp.path().join("home").join(USER);

        let err = mount_data_operation_impl(
            DATA_PATH,
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &SystemRunner,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not exist"), "{msg}");
        assert!(msg.contains("RSE"), "{msg}");
    }

    // --- mount: file not present in /skadata ---

    #[test]
    fn mount_errors_when_file_not_staged_to_local_rse() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        fs::create_dir_all(&skadata).unwrap(); // skadata exists but file is absent
        let home = tmp.path().join("home").join(USER);

        let err = mount_data_operation_impl(
            DATA_PATH,
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &SystemRunner,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("random10MiB.bin"), "{msg}");
        assert!(msg.contains("not found") || msg.contains("staged"), "{msg}");
    }

    #[test]
    fn mount_errors_when_skadata_dir_exists_but_subdirectory_is_absent() {
        // skadata exists but the namespace subdirectory (daac/08/06) does not
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        fs::create_dir_all(&skadata).unwrap();
        let home = tmp.path().join("home").join(USER);

        let err = mount_data_operation_impl(
            "/daac/08/06/random10MiB.bin",
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &SystemRunner,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("random10MiB.bin"), "{msg}");
    }

    // --- mount: path edge cases ---

    #[test]
    fn mount_errors_on_path_with_no_filename() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        fs::create_dir_all(&skadata).unwrap();
        let home = tmp.path().join("home").join(USER);

        // A path of "/" has no file_name component
        let err = mount_data_operation_impl(
            "/",
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &SystemRunner,
        )
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("invalid"), "{err}");
    }

    #[test]
    fn mount_errors_on_empty_data_path() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        fs::create_dir_all(&skadata).unwrap();
        let home = tmp.path().join("home").join(USER);

        let err = mount_data_operation_impl(
            "",
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &SystemRunner,
        )
        .unwrap_err();
        // An empty string has no file_name
        assert!(err.to_string().to_lowercase().contains("invalid"), "{err}");
    }

    // --- mount: golden path ---

    #[test]
    fn mount_golden_path() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        seed_skadata(&skadata);
        let home = tmp.path().join("home").join(USER);

        mount_data_operation_impl(
            DATA_PATH,
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &MockRunner::new(),
        )
        .unwrap();

        let bind_dir = home.join(".binds").join(NAMESPACE).join("random10MiB");
        let projects_file = home
            .join("projects")
            .join(NAMESPACE)
            .join("random10MiB.bin");
        assert!(bind_dir.exists(), "bind_dir should have been created");
        assert!(
            projects_file.exists(),
            "projects_file should have been created"
        );
    }

    // --- mount: chown safety ---

    /// Regression test: mounting a second file in the same namespace must succeed even when
    /// the `projects/<namespace>` directory already exists and contains a read-only placeholder
    /// file left over from the first mount.
    #[test]
    fn mount_second_file_in_same_namespace_succeeds() {
        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        let home = tmp.path().join("home").join(USER);

        // Seed two different files under the same skadata directory / namespace.
        let data_dir = skadata.join("daac/08/06");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("random10MiB.bin"), b"").unwrap();
        fs::write(data_dir.join("other100MiB.bin"), b"").unwrap();

        // First mount succeeds.
        mount_data_operation_impl(
            "/daac/08/06/random10MiB.bin",
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &MockRunner::new(),
        )
        .unwrap();

        // Simulate the projects_dir placeholder from the first mount being read-only
        // (as it would be after a real `mount --bind`).
        let first_projects_file = home.join("projects").join(NAMESPACE).join("random10MiB.bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&first_projects_file).unwrap().permissions();
            perms.set_mode(0o000); // no permissions — simulates a read-only bind mount
            fs::set_permissions(&first_projects_file, perms).unwrap();
        }

        // Second mount with a different file in the same namespace must not error.
        mount_data_operation_impl(
            "/daac/08/06/other100MiB.bin",
            NAMESPACE,
            USER,
            home.to_str().unwrap(),
            &skadata,
            &MockRunner::new(),
        )
        .unwrap();
    }

    // --- unmount: nothing mounted ---

    #[test]
    fn unmount_succeeds_when_nothing_is_mounted() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        // Neither bind_dir nor projects_file exist — should succeed gracefully.
        unmount_operation_impl(&home.join(USER), NAMESPACE, DATA_FILE).unwrap();
    }

    // --- unmount: cleanup ---

    #[test]
    fn unmount_removes_bind_dir_and_projects_file() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let base_path = home.join(USER);
        let bind_dir = base_path.join(".binds").join(NAMESPACE).join("random10MiB");
        let projects_dir = base_path.join("projects").join(NAMESPACE);
        let projects_file = projects_dir.join(DATA_FILE);

        fs::create_dir_all(&bind_dir).unwrap();
        fs::create_dir_all(&projects_dir).unwrap();
        fs::write(&projects_file, b"").unwrap();

        unmount_operation_impl(&base_path, NAMESPACE, DATA_FILE).unwrap();

        assert!(!bind_dir.exists(), "bind_dir should have been removed");
        assert!(
            !projects_file.exists(),
            "projects_file should have been removed"
        );
    }

    #[test]
    fn unmount_succeeds_when_only_bind_dir_exists() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let base_path = home.join(USER);
        let bind_dir = base_path.join(".binds").join(NAMESPACE).join("random10MiB");
        fs::create_dir_all(&bind_dir).unwrap();

        unmount_operation_impl(&base_path, NAMESPACE, DATA_FILE).unwrap();

        assert!(!bind_dir.exists(), "bind_dir should have been removed");
    }

    #[test]
    fn unmount_succeeds_when_only_projects_file_exists() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let base_path = home.join(USER);
        let projects_dir = base_path.join("projects").join(NAMESPACE);
        let projects_file = projects_dir.join(DATA_FILE);
        fs::create_dir_all(&projects_dir).unwrap();
        fs::write(&projects_file, b"").unwrap();

        unmount_operation_impl(&base_path, NAMESPACE, DATA_FILE).unwrap();

        assert!(
            !projects_file.exists(),
            "projects_file should have been removed"
        );
    }

    #[test]
    fn unmount_leaves_other_files_in_projects_dir_intact() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let base_path = home.join(USER);
        let projects_dir = base_path.join("projects").join(NAMESPACE);
        let target_file = projects_dir.join(DATA_FILE);
        let other_file = projects_dir.join("other_file.bin");
        fs::create_dir_all(&projects_dir).unwrap();
        fs::write(&target_file, b"").unwrap();
        fs::write(&other_file, b"untouched").unwrap();

        unmount_operation_impl(&base_path, NAMESPACE, DATA_FILE).unwrap();

        assert!(!target_file.exists(), "target file should be removed");
        assert!(
            other_file.exists(),
            "other file in same dir should be untouched"
        );
    }

    // --- integration tests (require root on Linux) ---
    //
    // These tests create a real system user to exercise actual `chown` behaviour —
    // verifying ownership changes on disk rather than just the shape of command arguments.
    //
    // They are marked `#[ignore]` and are skipped at runtime when not running as root.
    //
    // Run locally:
    //   docker build -f Dockerfile.test -t pf-test . && docker run --rm pf-test
    //
    // Run in CI: the `integration-test` job in .github/workflows/ci.yml runs them automatically.

    use std::sync::Mutex;

    /// Serialises integration tests that share the `pf_testuser` system user,
    /// preventing conflicts when the test harness runs tests in parallel.
    static INTEGRATION_MUTEX: Mutex<()> = Mutex::new(());

    /// RAII guard: creates the `pf_testuser` system user on construction and
    /// removes it via `userdel` on drop, even if the test panics.
    struct TestUser;

    impl TestUser {
        const NAME: &'static str = "pf_testuser";

        /// Creates the test user, panicking if it already exists (leftover from a prior run).
        fn create() -> Self {
            if users::get_user_by_name(Self::NAME).is_some() {
                panic!(
                    "test user '{}' already exists on this system — \
                     remove it first with: userdel {}",
                    Self::NAME,
                    Self::NAME,
                );
            }
            let status = Command::new("useradd")
                .args(["--no-create-home", "--system", Self::NAME])
                .status()
                .expect("failed to execute useradd — is it installed?");
            assert!(
                status.success(),
                "useradd failed with exit status: {status}"
            );
            Self
        }
    }

    impl Drop for TestUser {
        fn drop(&mut self) {
            let _ = Command::new("userdel").arg(Self::NAME).status();
        }
    }

    /// Runner that executes `chown` for real (so ownership changes are visible on disk)
    /// but mocks `bindfs` and `mount` to avoid needing those tools or mount privileges.
    ///
    /// All `run_command` calls are recorded so tests can inspect what was (or was not) invoked.
    struct CapturingRealChownRunner {
        commands: std::cell::RefCell<Vec<(String, Vec<String>)>>,
        mountpoint_calls: Cell<usize>,
    }

    impl CapturingRealChownRunner {
        fn new() -> Self {
            Self {
                commands: std::cell::RefCell::new(vec![]),
                mountpoint_calls: Cell::new(0),
            }
        }
    }

    impl Runner for CapturingRealChownRunner {
        fn run_command(&self, cmd: &str, args: &[&str], description: &str) -> Result<()> {
            self.commands.borrow_mut().push((
                cmd.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            if cmd == "chown" {
                run_command(cmd, args, description)
            } else {
                Ok(()) // mock bindfs and mount — no elevated mount privileges needed
            }
        }
        fn is_mountpoint(&self, _path: &Path) -> Result<bool> {
            let n = self.mountpoint_calls.get();
            self.mountpoint_calls.set(n + 1);
            Ok(n > 0)
        }
    }

    /// Verifies that after a first mount the bind and projects directories are actually
    /// owned by the target user on disk (uid and gid match), not just that a `chown`
    /// command was issued with the right arguments.
    #[test]
    #[ignore = "requires root on Linux; run via `docker run --rm pf-test` or the CI integration-test job"]
    fn integration_chown_sets_correct_ownership_on_first_mount() {
        if unsafe { libc::getuid() } != 0 {
            eprintln!("skipped: not running as root");
            return;
        }

        let _guard = INTEGRATION_MUTEX.lock().unwrap();
        let _user = TestUser::create();

        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        seed_skadata(&skadata);
        let home = tmp.path().join("home").join(TestUser::NAME);

        mount_data_operation_impl(
            DATA_PATH,
            NAMESPACE,
            TestUser::NAME,
            home.to_str().unwrap(),
            &skadata,
            &CapturingRealChownRunner::new(),
        )
        .unwrap();

        let bind_dir = home.join(".binds").join(NAMESPACE).join("random10MiB");
        let projects_dir = home.join("projects").join(NAMESPACE);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let user = users::get_user_by_name(TestUser::NAME).unwrap();
            let bind_meta = fs::metadata(&bind_dir).unwrap();
            let proj_meta = fs::metadata(&projects_dir).unwrap();
            assert_eq!(bind_meta.uid(), user.uid(), "bind_dir uid mismatch");
            assert_eq!(
                bind_meta.gid(),
                user.primary_group_id(),
                "bind_dir gid mismatch"
            );
            assert_eq!(proj_meta.uid(), user.uid(), "projects_dir uid mismatch");
            assert_eq!(
                proj_meta.gid(),
                user.primary_group_id(),
                "projects_dir gid mismatch"
            );
        }
    }

    /// Verifies that the placeholder file created inside `projects/<namespace>/` is owned
    /// by the target user, not by root.
    ///
    /// The file is created by the process running as root (via `fs::OpenOptions`), so
    /// without an explicit `chown` it would be root:root — inaccessible to the target user.
    #[test]
    #[ignore = "requires root on Linux; run via `docker run --rm pf-test` or the CI integration-test job"]
    fn integration_projects_placeholder_file_is_owned_by_target_user() {
        if unsafe { libc::getuid() } != 0 {
            eprintln!("skipped: not running as root");
            return;
        }

        let _guard = INTEGRATION_MUTEX.lock().unwrap();
        let _user = TestUser::create();

        let tmp = TempDir::new().unwrap();
        let skadata = tmp.path().join("skadata");
        seed_skadata(&skadata);
        let home = tmp.path().join("home").join(TestUser::NAME);

        mount_data_operation_impl(
            DATA_PATH,
            NAMESPACE,
            TestUser::NAME,
            home.to_str().unwrap(),
            &skadata,
            &CapturingRealChownRunner::new(),
        )
        .unwrap();

        let projects_file = home
            .join("projects")
            .join(NAMESPACE)
            .join("random10MiB.bin");

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let user = users::get_user_by_name(TestUser::NAME).unwrap();
            let file_meta = fs::metadata(&projects_file).unwrap();
            assert_eq!(
                file_meta.uid(),
                user.uid(),
                "projects placeholder file uid should be {}, got {}",
                user.uid(),
                file_meta.uid()
            );
            assert_eq!(
                file_meta.gid(),
                user.primary_group_id(),
                "projects placeholder file gid should be {}, got {}",
                user.primary_group_id(),
                file_meta.gid()
            );
        }
    }
}
