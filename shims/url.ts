declare function ts_url_parse(href: string, base: string): any;
declare function ts_url_resolve(base: string, relative: string): string;
declare function ts_url_format(obj: any): string;

export class URL {
  href: string;
  protocol: string;
  username: string;
  password: string;
  host: string;
  hostname: string;
  port: string;
  pathname: string;
  search: string;
  hash: string;
  origin: string;

  constructor(input: string, base?: string) {
    const parsed = ts_url_parse(input, base || '');
    if (!parsed) throw new TypeError('Invalid URL: ' + input);
    this.href = parsed.href || '';
    this.protocol = parsed.protocol || '';
    this.username = parsed.username || '';
    this.password = parsed.password || '';
    this.host = parsed.host || '';
    this.hostname = parsed.hostname || '';
    this.port = parsed.port || '';
    this.pathname = parsed.pathname || '/';
    this.search = parsed.search || '';
    this.hash = parsed.hash || '';
    this.origin = parsed.origin || '';
  }

  toString(): string { return this.href; }
  toJSON(): string { return this.href; }

  get searchParams(): any {
    const qs = this.search.startsWith('?') ? this.search.substring(1) : this.search;
    return new URLSearchParams(qs);
  }
}

export function resolve(from: string, to: string): string {
  return ts_url_resolve(from, to);
}

export function format(urlObj: any): string {
  if (typeof urlObj === 'string') return urlObj;
  return ts_url_format(urlObj);
}

export function parse(urlStr: string, _parseQueryString?: boolean, _slashesDenoteHost?: boolean): any {
  return ts_url_parse(urlStr, '');
}

const urlModule = { URL, resolve, format, parse };
export default urlModule;
