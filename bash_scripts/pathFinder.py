#!/usr/bin/env python3
import argparse
import os
import sys
import subprocess

def run(cmd, check=True):
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if check and res.returncode != 0:
        print(f"Error running: {' '.join(cmd)}\n{res.stderr.strip()}", file=sys.stderr)
        sys.exit(res.returncode)
    return res

def is_mountpoint(path):
    return subprocess.run(['mountpoint', '-q', path]).returncode == 0

def main():
    p = argparse.ArgumentParser(prog=os.path.basename(__file__), add_help=False)
    p.add_argument('option', nargs='?')
    p.add_argument('fits', nargs='?')
    p.add_argument('sudo_group', nargs='?')
    args = p.parse_args()

    if args.option not in ('--mount', '--unmount'):
        print(f"Usage: {os.path.basename(__file__)} [--mount|--unmount] <fits-path> <sudo-group>")
        sys.exit(1)

    if not args.fits or not args.sudo_group:
        print("Error: missing <fits-path> or <sudo-group>", file=sys.stderr)
        sys.exit(1)

    sudo_user = os.environ.get('SUDO_USER')
    if not sudo_user:
        print("Error: SUDO_USER not set. Run via sudo.", file=sys.stderr)
        sys.exit(1)

    fits = args.fits
    fits_file = os.path.basename(fits)
    fits_path = os.path.dirname(fits)  # may be ''
    bind_name = os.path.splitext(fits_file)[0]

    home = f"/home/{sudo_user}"
    bind_dir = os.path.join(home, '.binds', bind_name)
    projects_dir = os.path.join(home, 'projects')
    projects_file = os.path.join(projects_dir, fits_file)
    skadata_src = os.path.join('/skadata', args.sudo_group, fits_path)

    if args.option == '--mount':
        # avoid cyclic mounts
        if is_mountpoint(bind_dir):
            print(f"Error: {bind_dir} is already mounted; aborting to avoid cyclic mounts.", file=sys.stderr)
            sys.exit(1)

        os.makedirs(bind_dir, exist_ok=True)
        os.makedirs(projects_dir, exist_ok=True)

        # touch project file
        open(projects_file, 'a').close()

        # set ownership and perms
        run(['chown', '-R', f'{sudo_user}:{sudo_user}', os.path.join(home, '.binds')])
        run(['chmod', '600', bind_dir])  # file-like perms in original; keep simple
        run(['chown', '-R', f'{sudo_user}:{sudo_user}', projects_dir])
        run(['chmod', '500', projects_file])

        # bindfs then bind mount
        run(['bindfs', '--perms=0700', f'--force-user={sudo_user}', f'--force-group={sudo_user}', skadata_src, bind_dir])
        run(['mount', '--bind', os.path.join(bind_dir, fits_file), projects_file])

        # verify
        if is_mountpoint(projects_file):
            print(f"Mount verification successful: {fits_file} is mounted at {projects_file}")
        else:
            print(f"Error: Mount verification failed for {fits_file} at {projects_file}", file=sys.stderr)
            sys.exit(1)

    elif args.option == '--unmount':
        run(['umount', projects_file], check=False)
        run(['umount', bind_dir], check=False)
        run(['rm', '-rf', bind_dir])
        run(['rm', '-f', projects_file])
        print(f"Unmounted {fits_file} from {projects_file}")

if __name__ == '__main__':
    main()
# EOF