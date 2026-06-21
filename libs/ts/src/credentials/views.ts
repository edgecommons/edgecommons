/** Typed credential views — thin parses over an opaque Secret (canonical camelCase JSON). */
export interface AwsCredentials {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken?: string;
  expiry?: string;
}

export interface BasicAuth {
  username: string;
  password: string;
}

export interface TlsBundle {
  certPem: string;
  keyPem: string;
  caPem?: string;
}

export interface KafkaSasl {
  mechanism: string;
  username: string;
  password: string;
}
