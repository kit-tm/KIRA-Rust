#!/bin/bash
# This is a service that sends DNS Update Queries to a local DNS Server
#
# It registers under its own hostname (e.g. r2kad-n1) in the zone kira.internal (or the env variable KIRA_DNS_ZONE) 
# In the default settings this results in an A and an AAAA to r2kad-n1.kira.internal 
# Further names are read from the environment variable DNS_NAMES (separated by colons)
# It uses the NODE_IP attached to the kira network interface for the AAAA record as well as the IPv4 stored in the KIRA_IPV4 env variable


# Get the randomly generated IPv6 NODE_IP
AAAA=$(ip a | grep -oP '(?<=inet6 )fc00[0-9a-f:]*' | head -1)
A=$KIRA_IPV4

DNS_ZONE="${KIRA_DNS_ZONE:-kira.internal}"

# Build all names
NAMES="${HOSTNAME}.${DNS_ZONE}"

IFS=:
for NAME in ${DNS_NAMES}; do 
    NAMES="${NAMES} ${NAME}.${DNS_ZONE}" 
done


IFS=' '

# Send all updates
for NAME in $NAMES; do
    # Add AAAA record
    echo "server 127.0.0.1
    zone $DNS_ZONE
    update add ${NAME} 86400 AAAA ${AAAA}
    show
    send
    " | nsupdate

    # Add A record if we have an IPv4 Address
    if ! [ -z "$A" ]; then
        echo "server 127.0.0.1
        zone $DNS_ZONE
        update add ${NAME} 86400 A ${A}
        show
        send
        " | nsupdate
    fi
done





