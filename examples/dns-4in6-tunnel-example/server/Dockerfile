FROM kira-example-base

RUN apt install -y python3-fastapi python3-hypercorn
# COPY conf file and executable
COPY server.conf /etc/supervisor/conf.d
COPY server.py server.py
