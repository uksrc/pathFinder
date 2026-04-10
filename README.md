# Path Finder

**pathFinder** is a tool for mounting SKA data on Slurm clusters without copying the data locally. Currently, it is provided as a single binary or an RPM installer.

The tool allows the Scientist to specify which files, identified from the Science Gateway, they want to mount while keeping the files secure and owned by them. Two methods are planned: interactive and a workflow managed by the Science Gateway via prepareData.

Features:

- OAuth2 device code flow authentication
- Data location lookup via Data Management API
- Site capabilities verification via Site Capabilities API
- Secure data mounting with proper permissions

## Installation

For instructions on the requirements and setup for your HPC server environment, and installation of **pathFinder** itself, see the [SERVER-CONFIGURATION.md](./SERVER-CONFIGURATION.md) doc.

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

#### Authentication with SKA-IAM flow

Example using SKA-IAM OAuth2:

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

### Unmount Data

Example for unmounting a file:

    $ sudo pathFinder --namespace daac --file-name simple_file.txt --unmount
    Unmounted simple_file.txt from /home/sm2921/projects/daac/simple_file.txt

## Development

Notes on how to build the executable, run the unit and integration tests, and local Docker-based testing can be found in the development can be found in the [DEVELOPMENT.md](DEVELOPMENT.md) doc.

