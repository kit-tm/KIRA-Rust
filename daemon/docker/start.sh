#!/bin/sh
until [ -f /r2kad-daemon/start ]
do
     sleep 5
done

./r2kad-daemon
