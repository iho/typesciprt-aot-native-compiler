export function stringify(obj: any, sep?: string, eq?: string, _options?: any): string {
  const s = sep || '&';
  const e = eq || '=';
  const keys: string[] = Object.keys(obj);
  const parts: string[] = [];
  for (let i = 0; i < keys.length; i++) {
    const k = keys[i];
    const v = obj[k];
    if (v === null || v === undefined) continue;
    parts.push(encodeURIComponent(k) + e + encodeURIComponent(String(v)));
  }
  return parts.join(s);
}

export function parse(str: string, sep?: string, eq?: string, _options?: any): any {
  const s = sep || '&';
  const e = eq || '=';
  const result: any = {};
  if (!str || str.length === 0) return result;
  const parts = str.split(s);
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    if (!part) continue;
    const idx = part.indexOf(e);
    if (idx >= 0) {
      const k = decodeURIComponent(part.substring(0, idx));
      const v = decodeURIComponent(part.substring(idx + 1));
      if (result[k] === undefined) {
        result[k] = v;
      } else if (typeof result[k] === 'string') {
        result[k] = [result[k], v];
      } else {
        result[k].push(v);
      }
    } else {
      result[decodeURIComponent(part)] = '';
    }
  }
  return result;
}

export const escape = encodeURIComponent;
export const unescape = decodeURIComponent;

const querystring = { stringify, parse, escape, unescape };
export default querystring;
