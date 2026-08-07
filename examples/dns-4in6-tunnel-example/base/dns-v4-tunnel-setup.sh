#!/bin/bash
# This script takes a set of hostnames (in ENV "V4_TUNNEL_HOSTS" separated by colons) and sets up a 4in6 tunnel to each of them
#
# Prerequisites: local IPv4 in KIRA_IPV4
#

LOCAL_V6=$(ip a | grep -oP '(?<=inet6 )fc00[0-9a-f:]*' | head -1)
LOCAL_V4=$KIRA_IPV4

IFS=:
for NAME in $V4_TUNNEL_HOSTS; do
    REMOTE_V4=$(dig +short $NAME A)
    REMOTE_V6=$(dig +short $NAME AAAA)

    TUNNEL_NAME="tunnel-$(echo $NAME | cut -d'.' -f1)"
    echo "Setting up 4in6 tunnel from $LOCAL_V4 -> $LOCAL_V6 -> $REMOTE_V6 -> $REMOTE_V4"
    echo "ip tunnel add ${TUNNEL_NAME} mode ip4ip6 local ${LOCAL_V6} remote ${REMOTE_V6}"
    ip tunnel add "${TUNNEL_NAME}" mode ip4ip6 local "${LOCAL_V6}" remote "${REMOTE_V6}" \
    && ip link set dev ${TUNNEL_NAME} up \
    && ip addr add ${LOCAL_V4}/32 dev ${TUNNEL_NAME} \
    && ip route add ${REMOTE_V4}/32 dev ${TUNNEL_NAME}
done
