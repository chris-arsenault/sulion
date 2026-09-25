export interface AuthRuntimeConfig {
  cognitoUserPoolId: string;
  cognitoClientId: string;
}

declare global {
  interface Window {
    __APP_CONFIG__?: {
      cognitoUserPoolId?: string;
      cognitoClientId?: string;
      publicUploadOrigin?: string;
    };
  }
}

export function getAuthRuntimeConfig(): AuthRuntimeConfig | null {
  const raw = window.__APP_CONFIG__;
  if (!raw?.cognitoUserPoolId || !raw?.cognitoClientId) return null;
  return {
    cognitoUserPoolId: raw.cognitoUserPoolId,
    cognitoClientId: raw.cognitoClientId,
  };
}
