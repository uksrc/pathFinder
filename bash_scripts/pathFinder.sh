#!/bin/bash
OPTION=$1
FITS=$2
SUDO_GROUP=$3

FITS_FILE=$(echo $FITS | awk -F"/" '{print $NF}')
FITS_PATH=$(echo $FITS | awk -F"/" 'BEGIN {OFS = FS} {$(NF--)=""; print}')
FITS_GROUP=$(echo $FITS | awk -F"/" '{print $1}')
BIND_PATH=$(echo $FITS_FILE | awk -F"." '{print $1}')

if [ "$OPTION" = "--mount" ]; then
	# Check if the .binds target is already a mount to avoid cyclical mounts
	if mountpoint -q "/home/$SUDO_USER/.binds/$FITS_PATH"; then
		echo "Error: /home/$SUDO_USER/.binds/$FITS_PATH is already mounted; aborting to avoid cyclic mounts."
		exit 1
    else
    # Verify that the provided sudo group matches the namespace
        if [ $FITS_GROUP != "$SUDO_GROUP" ]; then
            echo "Error: Provided sudo group '$SUDO_GROUP' does not match fits group '$FITS_GROUP'; aborting."
            exit 1
        else
            mkdir -p "/home/$SUDO_USER/.binds/$BIND_PATH"
            chown -R "$SUDO_USER:$SUDO_USER" "/home/$SUDO_USER/.binds/"
            chmod 600 "/home/$SUDO_USER/.binds/$BIND_PATH"

            mkdir -p "/home/$SUDO_USER/projects"
            touch "/home/$SUDO_USER/projects/$FITS_FILE"
            chown -R "$SUDO_USER:$SUDO_USER" "/home/$SUDO_USER/projects/"
            chmod 600 "/home/$SUDO_USER/projects/$FITS_FILE"
            bindfs --perms=0700 --force-user="$SUDO_USER" --force-group="$SUDO_USER" "/skadata/$FITS_PATH" "/home/$SUDO_USER/.binds/$BIND_PATH"
            mount --bind "/home/$SUDO_USER/.binds/$BIND_PATH/$FITS_FILE" "/home/$SUDO_USER/projects/$FITS_FILE"
        fi
	fi
    # Verify the mount was successful
    if mountpoint -q "/home/$SUDO_USER/projects/$FITS_FILE"; then
        echo "Mount verification successful: $FITS_FILE is mounted at /home/$SUDO_USER/projects/$FITS_FILE"
    else
        echo "Error: Mount verification failed for $FITS_FILE at /home/$SUDO_USER/projects/$FITS_FILE"
        exit 1
    fi
elif [ "$OPTION" = "--unmount" ]; then
    umount "/home/$SUDO_USER/projects/$FITS_FILE"
    umount "/home/$SUDO_USER/.binds/$BIND_PATH"
    rm -rf "/home/$SUDO_USER/.binds/$BIND_PATH"
    rm -f "/home/$SUDO_USER/projects/$FITS_FILE"
    echo "Unmounted $FITS_FILE from /home/$SUDO_USER/projects/$FITS_FILE"
else
	echo "Usage: $0 [--mount|--unmount] <fits-path> <sudo-group>"
	exit 1
fi
