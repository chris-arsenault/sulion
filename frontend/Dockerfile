FROM docker.io/library/nginx:alpine
COPY dist/ /usr/share/nginx/html
COPY nginx.conf /etc/nginx/templates/nginx.conf.template
COPY docker-entrypoint.d/40-sulion-config.sh /docker-entrypoint.d/40-sulion-config.sh
ENV SULION_BACKEND_UPSTREAM=backend:8080 \
    SULION_BROKER_UPSTREAM=broker:8081 \
    SULION_RETRIEVAL_UPSTREAM=retrieval:8083 \
    NGINX_ENVSUBST_FILTER=^SULION_ \
    NGINX_ENVSUBST_OUTPUT_DIR=/etc/nginx
RUN chmod +x /docker-entrypoint.d/40-sulion-config.sh
EXPOSE 80
