declare function ts_os_platform(): string;
declare function ts_os_homedir(): string;
declare function ts_os_tmpdir(): string;
declare function ts_os_hostname(): string;
declare function ts_os_eol(): string;
declare function ts_os_arch(): string;
declare function ts_os_cpus(): any[];

export function platform(): string { return ts_os_platform(); }
export function homedir(): string { return ts_os_homedir(); }
export function tmpdir(): string { return ts_os_tmpdir(); }
export function hostname(): string { return ts_os_hostname(); }
export function arch(): string { return ts_os_arch(); }
export function cpus(): any[] { return ts_os_cpus(); }

export const EOL: string = ts_os_eol();

const os = { platform, homedir, tmpdir, hostname, arch, cpus, EOL };
export default os;
