declare function ts_readline_question(prompt: string): string;
declare function ts_readline_read_line(): string;

class Interface {
  private _input: any;
  private _output: any;
  private _closed: boolean;

  constructor(options: any) {
    this._input = options.input || null;
    this._output = options.output || null;
    this._closed = false;
  }

  question(prompt: string, callback: (answer: string) => void): void {
    if (this._closed) return;
    const answer = ts_readline_question(prompt);
    callback(answer);
  }

  close(): void {
    this._closed = true;
  }

  on(_event: string, _cb: any): this { return this; }
  once(_event: string, _cb: any): this { return this; }
}

export function createInterface(options: any): Interface {
  return new Interface(options);
}

export { Interface };

const readline = { createInterface, Interface };
export default readline;
