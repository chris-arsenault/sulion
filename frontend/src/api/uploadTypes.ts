export interface UploadTarget {
  repo: string;
  workspaceId?: string;
}

export interface UploadGrant {
  url: string;
  headers: Record<string, string>;
  expires_in: number;
}

export interface UploadIntent {
  repo?: string;
  workspace_id?: string;
  directory: string;
  filename: string;
  size: number;
  checksum: string;
}

export interface GrantedUpload { id: string; grant: UploadGrant }
