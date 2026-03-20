export class StringDecoder {
  private _encoding: string;
  constructor(encoding?: string) {
    this._encoding = (encoding || 'utf8').toLowerCase();
  }
  write(buffer: any): string {
    if (typeof buffer === 'string') return buffer;
    if (buffer === null || buffer === undefined) return '';
    if (typeof buffer.toString === 'function') {
      return buffer.toString(this._encoding);
    }
    return String(buffer);
  }
  end(buffer?: any): string {
    if (buffer !== undefined && buffer !== null) return this.write(buffer);
    return '';
  }
}

export default StringDecoder;
