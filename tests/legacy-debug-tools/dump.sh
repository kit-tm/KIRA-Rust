#!/bin/bash

if [ -z "$1" ]; then
  echo "Usage: $0 <container_id_or_name>"
  exit 1
fi

CONTAINER=$1

docker exec -it $CONTAINER sh -c "tcpdump -i any -Z root 'proto gre or (icmp6 and not (icmp6[0] == 135 or icmp6[0] == 136))'"
