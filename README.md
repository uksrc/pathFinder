# Path Finder

A Rust implementation of the SKA path finder tool for authentication, locating & mounting data from the SKA storage system within a Slurm login host. It provides a single binary and an RPM installer.

Features:

- OAuth2 device code flow authentication
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Secure data mounting with proper permissions

## Installation

For instructions on the setup & requirements for your HPC Server environment, see the [server configuration doc](./SERVER-CONFIGURATION.md)

## Usage

### Mount Data

The `pathFinder` command is available to run on the CLI after logging into the Slurm login node. The command needs to be run as `sudo` because it is mounting data, users in the `pathfinder` group are granted `sudo` privileges to execute the `pathFinder` executable.

      $ sudo pathFinder --help

      A tool for finding SKA data paths for mounting purposes

      Usage: pathFinder [OPTIONS] --namespace <NAMESPACE> --file-name <FILE_NAME>

      Options:
            --namespace <NAMESPACE>  Namespace of the data
            --file-name <FILE_NAME>  Name of the data file
            --no-login               Do not use OAuth2 for authentication - use environment variables instead
            --unmount                Unmount previously mounted data instead of mounting
        -h, --help                   Print help

#### OAUTH Authentication

Example using SKAIAM OAuth2:

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

#### Authentication using environment variables

Example with environment variables (e.g. for automation):

      export DATA_MANAGEMENT_ACCESS_TOKEN="your_token_here"
      export SITE_CAPABILITIES_ACCESS_TOKEN="your_token_here"
      sudo pathFinder --namespace daac --file-name simple_file.txt --no-login

#### Unmounting Data

Example for unmounting a file:

      $ sudo pathFinder --namespace daac --file-name simple_file.txt --unmount
      Unmounted simple_file.txt from /home/sm2921/projects/daac/simple_file.txt

## Development

Notes on development can be found in the [development doc](DEVELOPMENT.md).
