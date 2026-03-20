declare function ts_dns_lookup(hostname: string): string;
declare function ts_dns_resolve(hostname: string, rrtype: string): string[];
declare function ts_dns_lookup_async(hostname: string): Promise<string>;
declare function ts_dns_resolve_async(hostname: string, rrtype: string): Promise<string[]>;

export function lookup(hostname: string, options: any, callback?: (err: any, address: string, family: number) => void): void {
  const cb = typeof options === 'function' ? options : callback;
  ts_dns_lookup_async(hostname).then((addr: string) => {
    if (cb) cb(null, addr, addr.includes(':') ? 6 : 4);
  });
}

export function resolve(hostname: string, rrtype: any, callback?: (err: any, addresses: string[]) => void): void {
  const type = typeof rrtype === 'string' ? rrtype : 'A';
  const cb = typeof rrtype === 'function' ? rrtype : callback;
  ts_dns_resolve_async(hostname, type).then((addrs: string[]) => {
    if (cb) cb(null, addrs);
  });
}

export function resolve4(hostname: string, callback: (err: any, addresses: string[]) => void): void {
  resolve(hostname, 'A', callback);
}

export function resolve6(hostname: string, callback: (err: any, addresses: string[]) => void): void {
  resolve(hostname, 'AAAA', callback);
}

export function resolveMx(hostname: string, callback: (err: any, addresses: any[]) => void): void {
  ts_dns_resolve_async(hostname, 'MX').then((addrs: string[]) => {
    if (callback) callback(null, addrs.map((a: string) => ({ exchange: a, priority: 10 })));
  });
}

export const promises = {
  lookup(hostname: string, _options?: any): Promise<{ address: string; family: number }> {
    return ts_dns_lookup_async(hostname).then((addr: string) => ({
      address: addr,
      family: addr.includes(':') ? 6 : 4,
    }));
  },
  resolve(hostname: string, rrtype?: string): Promise<string[]> {
    return ts_dns_resolve_async(hostname, rrtype || 'A');
  },
  resolve4(hostname: string): Promise<string[]> {
    return ts_dns_resolve_async(hostname, 'A');
  },
  resolve6(hostname: string): Promise<string[]> {
    return ts_dns_resolve_async(hostname, 'AAAA');
  },
};

const dns = { lookup, resolve, resolve4, resolve6, resolveMx, promises };
export default dns;
