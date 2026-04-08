# Path Finder

A Rust implementation of the SKA path finder tool for authentication, locating & mounting data from the SKA storage system within a Slurm login host.

## System Requirements

For instructions on the setup & requirements for your HPC Server side environment see [pathFinder - Server Configuration](./SERVER-CONFIGURATION.md)



## Usage

**Note**: The tool will automatically check if the file exists locally at the local RSE `/skadata`. If the file is not found locally, it will display the sites where the file is available and prompt you to ensure the data has been staged to your local site before mounting.

### Mount Data

The `pathFinder` command is available to run on the CLI after logging into the Slurm login node.  The command needs to be run as `sudo` because it is mounting data, users in the `pathfinder` group are granted `sudo` privileges to execute the `pathFinder` executable.

#### Usage

```
$ sudo pathFinder --help

A tool for finding SKA data paths for mounting purposes

Usage: pathFinder [OPTIONS] --namespace <NAMESPACE> --file-name <FILE_NAME>

Options:
      --namespace <NAMESPACE>  Namespace of the data
      --file-name <FILE_NAME>  Name of the data file
      --no-login               Do not use OAuth2 for authentication - use environment variables instead
      --unmount                Unmount previously mounted data instead of mounting
  -h, --help                   Print help
```

#### OAUTH Authentication

Example using SKAIAM OAuth2 (required).

```
$ sudo pathFinder --namespace daac --file-name simple_file.txt

Authenticating with OAuth2...
Cached tokens expired

ACTION REQUIRED:
    Open this URL in a browser and authenticate: https://ska-iam.stfc.ac.uk/device?user_code=KNIBUH

Waiting for authentication (timeout: 5 minutes)...
Tokens cached for 3600 seconds
Authentication successful!
RSE Path for file 'simple_file.txt' in namespace 'daac': /daac/14/66/simple_file.txt
Mount verification successful: simple_file.txt is mounted at /home/sm2921/projects/daac/simple_file.txt
```

#### Token Authentication

Example with environment variables (for automation):

```
$ export DATA_MANAGEMENT_ACCESS_TOKEN="your_token_here"
$ export SITE_CAPABILITIES_ACCESS_TOKEN="your_token_here"
$ sudo pathFinder --namespace daac --file-name simple_file.txt
```

#### Unmounting Data

Example for unmounting a file.

```
$ sudo pathFinder --namespace daac --file-name simple_file.txt --unmount
Unmounted simple_file.txt from /home/sm2921/projects/daac/simple_file.txt
```

#### RPM Package

The binary and RPM are built and published at [pathFinder GitHub release](https://github.com/uksrc/pathFinder/releases).

Check your current release with :

```
dnf info pathFinder
```



