declare function ts_event_emitter_new(): any;
declare function ts_event_emitter_on(emitter: any, event: string, listener: any): any;
declare function ts_event_emitter_once(emitter: any, event: string, listener: any): any;
declare function ts_event_emitter_off(emitter: any, event: string, listener: any): any;
declare function ts_event_emitter_emit(emitter: any, event: string, arg: any): boolean;
declare function ts_event_emitter_remove_all_listeners(emitter: any, event: any): any;
declare function ts_event_emitter_listeners(emitter: any, event: string): any[];

export class EventEmitter {
  private _ee: any;

  constructor() {
    this._ee = ts_event_emitter_new();
  }

  on(event: string, listener: (...args: any[]) => void): this {
    ts_event_emitter_on(this._ee, event, listener);
    return this;
  }

  once(event: string, listener: (...args: any[]) => void): this {
    ts_event_emitter_once(this._ee, event, listener);
    return this;
  }

  off(event: string, listener: (...args: any[]) => void): this {
    ts_event_emitter_off(this._ee, event, listener);
    return this;
  }

  removeListener(event: string, listener: (...args: any[]) => void): this {
    return this.off(event, listener);
  }

  addListener(event: string, listener: (...args: any[]) => void): this {
    return this.on(event, listener);
  }

  emit(event: string, ...args: any[]): boolean {
    return ts_event_emitter_emit(this._ee, event, args[0]);
  }

  removeAllListeners(event?: string): this {
    ts_event_emitter_remove_all_listeners(this._ee, event as any);
    return this;
  }

  listeners(event: string): Function[] {
    return ts_event_emitter_listeners(this._ee, event);
  }
}

export default EventEmitter;
