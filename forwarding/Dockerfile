FROM ubuntu:latest

RUN apt update
RUN apt install -y iproute2 nftables iputils-ping nano tcpdump net-tools

COPY nftables.conf /nftables.conf
COPY start.sh /start.sh
COPY localpath.sh /localpath.sh
COPY nodetopath.sh /nodetopath.sh
COPY pathtopath.sh /pathtopath.sh

RUN chmod +x /localpath.sh
RUN chmod +x /nodetopath.sh
RUN chmod +x /pathtopath.sh
RUN chmod +x /start.sh

ENTRYPOINT [ "/start.sh" ]
