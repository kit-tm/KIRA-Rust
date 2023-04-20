#!/bin/sh

nft add element ip6 kira nodeidtopathid {"$1" : "$2"}
nft add element ip6 kira encapsulate {"$1"}
