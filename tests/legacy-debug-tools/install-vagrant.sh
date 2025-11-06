#!/bin/bash

set -e
set -u
set -x

# fix annoying vagrant problem w/ missing locales
locale-gen en_US.UTF-8
export LC_ALL="en_US.UTF-8"
export LC_CTYPE="en_US.UTF-8"
echo 'export LC_ALL="en_US.UTF-8"' >> /home/vagrant/.bashrc
echo 'export LC_CTYPE="en_US.UTF-8"' >> /home/vagrant/.bashrc

# rename host to kira
hostnamectl set-hostname "kira"

# install rsync for pushing changes from local computer to vm
apt-get -y update && apt-get -y install rsync

# install.
# installs containernet and openvswitch to "/root"
cd /home/vagrant/vagrant/


sudo apt -y install \
    ansible \
    autoconf \
    build-essential \
    ca-certificates \
    cmake \
    curl \
    git \
    software-properties-common \
    python3 \
    python3-pip \
    python3-venv \
    python3-networkx \
    sysstat \
    uuid-runtime

# install containernet
pushd /root
git clone https://github.com/containernet/containernet.git
sudo ansible-playbook -i "localhost," -c local containernet/ansible/install.yml
popd

# build docker image
make build-image-supervisord
