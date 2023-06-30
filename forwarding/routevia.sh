#!/bin/sh
ip -6 route add $1/128 via $2
