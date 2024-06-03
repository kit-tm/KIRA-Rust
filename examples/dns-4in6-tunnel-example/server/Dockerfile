FROM kira-example-base

RUN apt install -y python3-fastapi python3-hypercorn
# COPY conf file and executable
COPY server.conf /etc/supervisord.d/server.conf
COPY server.py server.py
