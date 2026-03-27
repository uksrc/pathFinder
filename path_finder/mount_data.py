#!/usr/bin/env python3
"""
Python conversion of pathFinder.sh
Handles mounting and unmounting of RSE files with proper privilege management.
"""

import argparse
import os
import sys
import subprocess
import pwd
from pathlib import Path


def mount_unmount(rse_path: Path, namespace: str, mount: bool = True):
    """Main entry point for the script if used from the CLI."""
    check_root_privileges()
    sudo_user = get_sudo_user()

    if mount:
        mount_file(rse_path, namespace, sudo_user)
    else:
        unmount_file(rse_path, sudo_user)


def check_root_privileges():
    """Check if the script is running with root privileges."""
    if os.geteuid() != 0:
        print("Error: This script must be run with sudo privileges.")
        sys.exit(1)


def get_sudo_user():
    """Get the actual user who invoked sudo."""
    sudo_user = os.environ.get("SUDO_USER")
    if not sudo_user:
        print("Error: SUDO_USER environment variable not found.")
        sys.exit(1)
    return sudo_user


def mount_file(rse_path: Path, sudo_group: str, sudo_user: str):
    """Mount a file with proper bindings."""
    filename, filepath, namespace, bind_path = parse_rse_path(rse_path)

    verify_namespace_matches_group(namespace, sudo_group)

    bind_target, project_file = prepare_mount_paths(
        sudo_user, filepath, bind_path, filename
    )

    check_no_cyclical_mount(bind_target.parent)

    uid, gid = get_user_ids(sudo_user)
    create_mount_directories(sudo_user, bind_target, project_file, uid, gid)

    perform_bindfs_mount(filepath, bind_target, sudo_user)
    perform_bind_mount(bind_target, filename, project_file)

    verify_mount_success(project_file, filename)


def parse_rse_path(rse_path: Path) -> tuple[str, Path, str, Path]:
    """Parse the RSE path into source and target paths.

    Args:
        rse_path: Path - The RSE path to parse
    Returns:
        filename: str - The RSE filename
        filepath: Path - The directory path of the RSE file
        namespace: str - The group (first component of the path)
        bind_path: Path - The bind path without the file extension
    """
    validate_rse_path(rse_path)

    filename = rse_path.name
    namespace = rse_path.relative_to(rse_path.parents[-1]).parts[0]
    filepath = rse_path.parent
    bind_path = rse_path.with_suffix("")

    return filename, filepath, namespace, bind_path


def validate_rse_path(rse_path: Path):
    """Validate that the RSE path has the required structure."""
    if not rse_path.parts:
        print("Error: Invalid RSE path provided.")
        sys.exit(1)
    if len(rse_path.parts) < 2:
        print(
            "Error: RSE path must include at least a group and a file - ie. like group/path/to/filename"
        )
        sys.exit(1)


def verify_namespace_matches_group(namespace: str, sudo_group: str):
    """Verify that the provided sudo group matches the namespace."""
    if namespace != sudo_group:
        print(
            f"Error: Provided sudo group '{sudo_group}' does not match fits group '{namespace}'; aborting."
        )
        sys.exit(1)


def prepare_mount_paths(
    sudo_user: str, filepath: Path, bind_path: Path, filename: str
) -> tuple[Path, Path]:
    """Prepare and return the bind target and project file paths."""
    home_dir = f"/home/{sudo_user}"
    bind_target = Path(home_dir) / ".binds" / bind_path
    project_file = Path(home_dir) / "projects" / filename

    return bind_target, project_file


def check_no_cyclical_mount(binds_path: Path):
    """Check if the .binds target is already a mount to avoid cyclical mounts."""
    if is_mountpoint(binds_path):
        print(
            f"Error: {binds_path} is already mounted; aborting to avoid cyclic mounts."
        )
        sys.exit(1)


def is_mountpoint(path: Path) -> bool:
    """Check if a path is a mount point."""
    result = subprocess.run(["mountpoint", "-q", str(path)], capture_output=True)
    return result.returncode == 0


def get_user_ids(username: str) -> tuple[int, int]:
    """Get UID and GID for a given username."""
    try:
        pw_record = pwd.getpwnam(username)
        return pw_record.pw_uid, pw_record.pw_gid
    except KeyError:
        print(f"Error: User '{username}' not found.")
        sys.exit(1)


def create_mount_directories(
    sudo_user: str, bind_target: Path, project_file: Path, uid: int, gid: int
):
    """Create and configure the bind and project directories."""
    home_dir = f"/home/{sudo_user}"

    # Create and configure the bind directory
    bind_target.mkdir(parents=True, exist_ok=True)
    os.chown(Path(home_dir) / ".binds", uid, gid)
    os.chmod(bind_target, 0o600)

    # Create and configure the projects directory and file
    projects_dir = project_file.parent
    projects_dir.mkdir(parents=True, exist_ok=True)
    project_file.touch()
    os.chown(projects_dir, uid, gid)
    os.chmod(project_file, 0o600)


def perform_bindfs_mount(filepath: Path, bind_target: Path, sudo_user: str):
    """Perform bindfs mount from source to bind target."""
    source_path = f"/skadata/{filepath}"
    bindfs_cmd = [
        "bindfs",
        "--perms=0700",
        f"--force-user={sudo_user}",
        f"--force-group={sudo_user}",
        source_path,
        str(bind_target),
    ]

    result = subprocess.run(bindfs_cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Error: bindfs command failed: {result.stderr}")
        sys.exit(1)


def perform_bind_mount(bind_target: Path, filename: str, project_file: Path):
    """Perform bind mount from bind target to project file."""
    mount_cmd = ["mount", "--bind", str(bind_target / filename), str(project_file)]

    result = subprocess.run(mount_cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Error: mount command failed: {result.stderr}")
        # Cleanup: unmount bindfs
        subprocess.run(["umount", str(bind_target)], capture_output=True)
        sys.exit(1)


def verify_mount_success(project_file: Path, filename: str):
    """Verify the mount was successful."""
    if is_mountpoint(project_file):
        print(f"Mount verification successful: {filename} is mounted at {project_file}")
    else:
        print(f"Error: Mount verification failed for {filename} at {project_file}")
        sys.exit(1)


def unmount_file(rse_path: Path, sudo_user: str):
    """Unmount a RSE file and clean up."""
    filename, _filepath, _namespace, bind_path = parse_rse_path(rse_path)
    bind_target, project_file = prepare_mount_paths(
        sudo_user, _filepath, bind_path, filename
    )

    unmount_project_file(project_file)
    unmount_bind_target(bind_target)
    cleanup_mount_artifacts(bind_target, project_file)

    print(f"Unmounted {filename} from {project_file}")


def unmount_project_file(project_file: Path):
    """Unmount the project file."""
    result = subprocess.run(
        ["umount", str(project_file)], capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f"Warning: Failed to unmount {project_file}: {result.stderr}")


def unmount_bind_target(bind_target: Path):
    """Unmount the bind target."""
    result = subprocess.run(
        ["umount", str(bind_target)], capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f"Warning: Failed to unmount {bind_target}: {result.stderr}")


def cleanup_mount_artifacts(bind_target: Path, project_file: Path):
    """Clean up mount directories and files."""
    if bind_target.exists():
        subprocess.run(["rm", "-rf", str(bind_target)], capture_output=True)

    if project_file.exists():
        project_file.unlink()


if __name__ == "__main__":
    # Test mounting and unmounting functionality
    # Parse command-line arguments

    parser = argparse.ArgumentParser(
        description="Handle mounting and unmounting of RSE files with proper privilege management."
    )
    parser.add_argument(
        "option", choices=["--mount", "--unmount"], help="Operation to perform"
    )
    parser.add_argument("rse_path", help="Path to the RSE file")
    parser.add_argument(
        "namespace", help="namespace of the data - also corresponds to sudo group"
    )

    args = parser.parse_args()

    option = args.option
    rse_path = Path(args.rse_path)
    namespace = args.namespace
    mount_unmount(rse_path, namespace, mount=(option == "--mount"))
