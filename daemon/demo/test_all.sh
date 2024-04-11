#! /bin/bash

for i in {0..9}
do
    for o in {0..9}
    do
        docker exec ${PREFIX}${NAME:=r2kad-n}$i /usr/bin/ping -c 3 -i 0.25 -W 1 -q fc00::$o 2>&1 > /dev/null \
            || echo "$i -> $o"
    done
done
