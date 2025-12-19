# Path Finder

A CLI Program to authorise a users access to some srcNet data and return the RSE path.

## USE CASE

- A user can invoke this program via the CLI to request the RSE path for a given srcNet data object.
- The user could be authenticated by using OAuth or providing a token.
- The data object could be provided as an IVO URI, or as namespace & filename.
- The script will check that the user is authorised to read the data object.
- If the user is authorised, then the RSE location of the data is obtained.

### Extra use case info

The RSE location will be used to run a `bindfs` command on the parent folder to mount this into the user's `~/.skadata/` directory, setting the user and group to the current user.  The specific file from the parent folder will then be used to `mount --bind` that file to `~/skadata/[FILE_NAME]`.

