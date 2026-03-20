declare function ts_crypto_random_uuid(): string;
declare function ts_crypto_random_bytes_hex(size: number): string;
declare function ts_crypto_hash_sync(algorithm: string, data: string, encoding: string): string;
declare function ts_crypto_hmac_sync(algorithm: string, key: string, data: string, encoding: string): string;

export function randomUUID(): string { return ts_crypto_random_uuid(); }

class RandomBytesResult {
  private _hex: string;
  constructor(hex: string) { this._hex = hex; }
  toString(_enc?: string): string { return this._hex; }
}

export function randomBytes(size: number): RandomBytesResult {
  return new RandomBytesResult(ts_crypto_random_bytes_hex(size));
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

const crypto = { randomUUID, randomBytes, createHash, createHmac };
export default crypto;
