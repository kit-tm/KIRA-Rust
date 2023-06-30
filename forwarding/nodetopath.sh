#!/bin/sh
ip -6 route add $1/128 encap ip6 dst $2 dev kira