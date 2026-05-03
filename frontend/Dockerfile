FROM docker.io/library/nginx:alpine
COPY dist/ /usr/share/nginx/html
COPY nginx.conf /etc/nginx/nginx.conf
COPY docker-entrypoint.d/40-sulion-config.sh /docker-entrypoint.d/40-sulion-config.sh
RUN chmod +x /docker-entrypoint.d/40-sulion-config.sh
EXPOSE 80
