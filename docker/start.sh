#!/bin/sh
until [ -f /kirad/start ]
do
     sleep 5
done

./kirad
