# Server Configuration

This document covers the prerequisites to setup on the underlying configuration on a Slurm cluster using a Ceph file-system and the installation of the pathFinder tool.

## Pre-requisites

The following requirements must be met:

- CRB Enabled
- RHEL EPEL (Extra Packages)
- BindFS
- Ceph Common

Note: These are for Rocky 9.x releases and have not been tested on RHEL 10.x or Ubuntu.

## Server-Side Configuration

The configuration is only required on the Login node of your Slurm cluster, this assumes that all your user home directories are CephFS/NFS mount points.

If you already have EPEL enabled you can skip the next 2 steps.

1.  Enable CRB:

    crb status
    crb enable

2.  Install EPEL:

    sudo dnf install epel-release
    sudo dnf repolist

3.  Configure your Ceph Keyring:

        vi /etc/ceph/ceph.client.rucio_prod_ro.keyring

    ... and add your Access key:

        [client.rucio_prod_ro]
        key = ****************************

4.  Create mountpoints, this MUST be owned by root with permissions of 550 & 700:

    sudo mkdir /skadata /mnt/private_mounts/skadata
    sudo chmod 550 /skadata
    sudo chmod 700 /mnt/private_mounts/skadata

5.  Add the following entries to the **/etc/fstab** file:

        # Ceph mount
        10.4.200.9:6789,10.4.200.13:6789,10.4.200.13:6789,10.4.200.17:6789,10.4.200.25:6789,10.4.200.26:6789:/volumes/_nogroup/a8af40e8-6412-44da-ad08-3731fdf19258/4945e5c2-aab7-4416-9b75-666f2af512d7 /skadata ceph name=rucio_prod_ro,x-systemd.device-timeout=30,x-systemd.mount-timeout=30,noatime,_netdev,ro,nodev,nosuid 0 2

        # Bindfs mount
        /mnt/private_mounts/skadata /skadata fuse.bindfs force-user=root,force-group=root 0 0

    **Note**: The `/mnt/private_mounts` is used to hide the owner & group of the mounted file-system, so all files under `/skadata` are presented as `root root` for owner and group and hides the real owner **uid/gid** which would typically be the xrootd, Webdav & Storm user uid/gid.

6.  Mount the /skadata mountpoints:

    mount -a
    systemctl daemon-reload

    **Note**: We use bindfs here as well so all files under `/skadata` are presented as `root root` for owner and group and hides the real owner **uid/gid** which would typically be the xrootd, Webdav & Storm user uid/gid:

7.  Add a sudoers file to control access to the pathfinder tool:

    vi /etc/sudoers.d/pathFinder

    ...allow group `pathfinder` access to the `pathFinder` executable:

    %pathfinder ALL = NOPASSWD: /usr/bin/pathfinder, /usr/bin/pathFinder

8.  Add the patfinder group:

    groupadd pathfinder

9.  Add or update the local users to their corresponding group:

    usermod -a -G pathfinder sm2921

## RPM Package installation

An RPM installation package is published on the [pathFinder release](https://github.com/uksrc/pathFinder/releases) page, check for the latest version before installing or upgrading.

Set the version and install the pathFinder package:

    VERSION=1.x.y dnf upgrade https://github.com/uksrc/pathFinder/releases/download/v${VERSION}/pathfinder-${VERSION}-1.x86_64.rpm

## HTTP server RPM installation

An RPM that installs the `pathfinder-http` binary as a `systemd` daemon is also
published on the [pathFinder release](https://github.com/uksrc/pathFinder/releases)
page.

Set the version and install the HTTP server package:

    VERSION=2.0.0
    sudo dnf install https://github.com/uksrc/pathFinder/releases/download/v${VERSION}/pathfinder-http-${VERSION}-1.x86_64.rpm

The package installs:

- `/usr/bin/pathfinder-http`
- `/usr/lib/systemd/system/pathfinder-http.service`
- `/etc/default/pathfinder-http`
- `/var/lib/pathfinder-http/`

The install script enables and starts the service automatically. Edit
`/etc/default/pathfinder-http` to change the database path or the listen address.
