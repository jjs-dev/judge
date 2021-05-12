FROM ghcr.io/jjs-dev/pps-cli:latest
WORKDIR /usr/local/bin
COPY main.sh main.sh
VOLUME [ "/var/toolchains", "/var/problems", "/etc/toolchains", "/etc/problems" ]
ENTRYPOINT [ "/bin/bash", "/usr/local/bin/main.sh" ]
