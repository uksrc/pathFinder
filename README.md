# pathFinder #

pathFinder is a tool for mounting SKA data on Slurm clusters without copying the data locally.

It allows the Scientist to specify which files, identified from the Science Gateway, they want to mount while keeping the files secure and owned by them.
## TODO Development

- [ ] Always check site capabilities to ensure that the data is staged to your local RSE.
  - [ ] Work out whether we need to check for tier 0.
- [ ] Tidy up the code around checking the response from the DM API `data/locate` request.
- [ ] Use this script to perform the data mount.
- [ ] Investigate whether the data can be specified using the IVO URI.

## HOW TO Try this script during development

1. Ensure you have installed `uv` - <https://docs.astral.sh/uv/>

        uv --version

    NB., you can use other dependency managers which use the `pyproject.toml` - e.g. `poetry`. Hint: `uv` is way faster!

2. Set your Data Management API Access Token:

    1. Navigate to <https://gateway.srcnet.skao.int>
    2. Click your initials badge in the top-right and select "View Token"
    3. Copy the "Data management access token" string
    4. Set the DATA_MANAGEMENT_ACCESS_TOKEN environment variable in your shell:

            export DATA_MANAGEMENT_ACCESS_TOKEN=[PASTED STRING]

3. Run the script while `uv` takes care of the dependencies for you:

        uv run path_finder/path_finder.py

## USE CASE

Two methods are planned, interactive and a workflow managed by the Science Gateway via prepareData.

This documentation covers the prerequisites to setup on the underlying configuration on a Slurm cluster and the installation of the pathFinder tool.


## Pre-requisites ##

The following requirements must be met. 

(Note these are for Rocky 9.x releases and have not been tested on RHEL 10.x or Ubuntu)

    - CRB Enabled 
    - RHEL EPEL (Extra Packages)
    - BindFS
    - Ceph Common

## Server Side Configuration ##

The configuration is only required on the Login node of your Slurm cluster, this assumes that all your user home directories are CephFS/NFS mount points.

If you already have EPEL enabled you can skip the next 2 steps.

1. Enable CRB

```
crb status
crb enable
```

2. Install EPEL
```
sudo dnf install epel-release
sudo dnf repolist
```

3. Configure your Ceph Keyring

```
vi /etc/ceph/ceph.client.rucio_prod_ro.keyring
```
Add your Access key.
```
[client.rucio_prod_ro]
key = ****************************
```

4. Add an /etc/fstab entry
```
10.4.200.9:6789,10.4.200.13:6789,10.4.200.13:6789,10.4.200.17:6789,10.4.200.25:6789,10.4.200.26:6789:/volumes/_nogroup/a8af40e8-6412-44da-ad08-3731fdf19258/4945e5c2-aab7-4416-9b75-666f2af512d7 /skadata ceph name=rucio_prod_ro,x-systemd.device-timeout=30,x-systemd.mount-timeout=30,noatime,_netdev,ro,nodev,nosuid 0 2 
```
5. Mount the /skadata mountpoint.

Note that we use bindfs here as well so all files under `/skadata` are presented as `root root` for owner and group and hides the real owner **uid/gid** which would typically be the xrootd, Webdav & Storm user uid/gid.
```
mount /skadata
systemctl daemon-reload
bindfs -u root -g root /skadata /skadata
```

6. Create a mountpoint, this MUST be owned by root with permissions of 550.
```
sudo mkdir /skadata
sudo chmod 550 /skadata
```

7. Add a sudoers file to control access to the pathfinder tool. 
```
vi /etc/sudoers.d/pathFinder
```
Using group `pathfinder` for group access for users you want to give access to.
```
%pathfinder ALL = NOPASSWD: /usr/bin/pathfinder, /usr/bin/pathFinder
```

8. Add the local groups.
```
groupadd pathfinder
```

9. Add or update the local users to their corresponding group.
```
usermod -a -G pathfinder sm2921
```
10. Install the pathFinder package.
```
dnf install https://github.com/uksrc/pathFinder/releases/download/v1.0.0/pathfinder-1.0.0-1.x86_64.rpm
```



The RSE location will be used to run a `bindfs` command on the parent folder to mount this into the user's `~/.skadata/` directory, setting the user and group to the current user. The specific file from the parent folder will then be used to `mount --bind` that file to `~/skadata/[FILE_NAME]`.
