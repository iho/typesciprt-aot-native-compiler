declare function ts_performance_now(): number;
declare function ts_performance_mark(name: string): void;
declare function ts_performance_measure(name: string, startMark: string): any;
declare function ts_performance_get_entries_by_name(name: string): any[];

export const performance = {
  now(): number { return ts_performance_now(); },
  mark(name: string): void { ts_performance_mark(name); },
  measure(name: string, startMark?: string): any {
    return ts_performance_measure(name, startMark || '');
  },
  getEntriesByName(name: string, _type?: string): any[] {
    return ts_performance_get_entries_by_name(name);
  },
  getEntries(): any[] { return []; },
  getEntriesByType(_type: string): any[] { return []; },
  clearMarks(_name?: string): void {},
  clearMeasures(_name?: string): void {},
};

export class PerformanceObserver {
  private _callback: any;
  constructor(callback: any) { this._callback = callback; }
  observe(_options?: any): void {}
  disconnect(): void {}
}

export class PerformanceEntry {
  name: string;
  entryType: string;
  startTime: number;
  duration: number;
  constructor(name: string, type: string, start: number, duration: number) {
    this.name = name; this.entryType = type; this.startTime = start; this.duration = duration;
  }
}

const perfHooks = { performance, PerformanceObserver, PerformanceEntry };
export default perfHooks;
