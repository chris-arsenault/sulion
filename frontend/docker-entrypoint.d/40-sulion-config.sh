#!/bin/sh
set -eu

cat > /usr/share/nginx/html/config.js <<EOF
window.__APP_CONFIG__ = {
  cognitoUserPoolId: "${SULION_COGNITO_USER_POOL_ID:-}",
  cognitoClientId: "${SULION_COGNITO_CLIENT_ID:-}",
  publicUploadOrigin: "${SULION_PUBLIC_UPLOAD_ORIGIN:-}"
};
EOF
