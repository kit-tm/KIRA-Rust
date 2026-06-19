FROM kira:latest

# install service dependencies
RUN apt install -y python3 python3-requests jq python3-dnslib dnsutils python3-pip

# copy dns-server service
COPY dns-dht-resolver.conf /etc/supervisord.d/dns-dht-resolver.conf
COPY dns-dht-resolver.py dns-dht-resolver.py

# copy registration service
COPY dns-registration.conf /etc/supervisord.d/dns-registration.conf
COPY dns-registration.sh dns-registration.sh

# copy v4 tunnel setup script
COPY dns-v4-tunnel-setup.conf /etc/supervisord.d/dns-v4-tunnel-setup.conf
COPY dns-v4-tunnel-setup.sh dns-v4-tunnel-setup.sh
