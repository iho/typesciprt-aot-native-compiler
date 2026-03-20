declare function ts_crypto_random_uuid(): string;
declare function ts_crypto_random_bytes_hex(size: number): string;
declare function ts_crypto_random_bytes(size: number): any;
declare function ts_crypto_hash_sync(algorithm: string, data: string, encoding: string): string;
declare function ts_crypto_hmac_sync(algorithm: string, key: string, data: string, encoding: string): string;
declare function ts_crypto_pbkdf2_sync(password: any, salt: any, iterations: number, keylen: number, digest: string): any;
declare function ts_crypto_scrypt_sync(password: any, salt: any, keylen: number, options: any): any;
declare function ts_crypto_timing_safe_equal(a: any, b: any): boolean;
declare function ts_crypto_random_fill_sync(buf: any): any;

export function randomUUID(): string { return ts_crypto_random_uuid(); }

export function randomBytes(size: number): any {
  return ts_crypto_random_bytes(size);
}

class Hash {
  private _algo: string;
  private _data: string;
  constructor(algo: string) { this._algo = algo; this._data = ""; }
  update(data: string): Hash { this._data = this._data + data; return this; }
  digest(encoding?: string): string { return ts_crypto_hash_sync(this._algo, this._data, encoding || "hex"); }
}

class Hmac {
  private _algo: string;
  private _key: string;
  private _data: string;
  constructor(algo: string, key: string) { this._algo = algo; this._key = key; this._data = ""; }
  update(data: string): Hmac { this._data = this._data + data; return this; }
  digest(encoding?: string): string { return ts_crypto_hmac_sync(this._algo, this._key, this._data, encoding || "hex"); }
}

export function createHash(algorithm: string): Hash { return new Hash(algorithm); }
export function createHmac(algorithm: string, key: string): Hmac { return new Hmac(algorithm, key); }

export function pbkdf2Sync(password: string, salt: string, iterations: number, keylen: number, digest: string): any {
  return ts_crypto_pbkdf2_sync(password, salt, iterations, keylen, digest);
}
export function scryptSync(password: string, salt: string, keylen: number, options?: any): any {
  return ts_crypto_scrypt_sync(password, salt, keylen, options || {});
}
export function timingSafeEqual(a: any, b: any): boolean {
  return ts_crypto_timing_safe_equal(a, b);
}
export function randomFillSync(buf: any): any {
  return ts_crypto_random_fill_sync(buf);
}
export function getHashes(): string[] {
  return ['sha1', 'sha256', 'sha512', 'md5'];
}

const crypto = {
  randomUUID,
  randomBytes,
  createHash,
  createHmac,
  pbkdf2Sync,
  scryptSync,
  timingSafeEqual,
  randomFillSync,
  getHashes,
};
export default crypto;
