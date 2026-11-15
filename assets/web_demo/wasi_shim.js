// ブラウザ版 (WebAssembly) の plantwright のための、WASI の関数の小さな代わり。
//
// 形状カーネル SMLib (C++) は C の標準ライブラリ (wasi-libc) を使うので、出来上がった .wasm は
// `wasi_snapshot_preview1` の関数を呼ぶ。ブラウザにはそれが無いので、ここで足りる分だけ用意する。
// - 標準出力・標準エラー出力はブラウザのコンソールに出す
// - ファイルはメモリの中に持つ (形の受け渡しに使う一時ファイルだけで、ページを閉じれば消える)
// index.html の import map で `wasi_snapshot_preview1` をこのファイルに向ける。しくみは docs/web-demo.md

const ESUCCESS = 0;
const EBADF = 8;
const EINVAL = 28;
const ENOENT = 44;
const ENOSYS = 52;

const PREOPEN_FD = 3;
const files = new Map(); // パス → Uint8Array
const open = new Map(); // fd → { path, data, size, pos, append }
let nextFd = PREOPEN_FD + 1;
let memory = null;

// wasm-bindgen が作った JS はインスタンスのメモリを外に出さないので、作られたときに拾っておく
function capture(result) {
  const instance = result.instance ?? result;
  if (instance.exports && instance.exports.memory) memory = instance.exports.memory;
  return result;
}
const instantiate = WebAssembly.instantiate;
WebAssembly.instantiate = (...args) => instantiate(...args).then(capture);
if (WebAssembly.instantiateStreaming) {
  const instantiateStreaming = WebAssembly.instantiateStreaming;
  WebAssembly.instantiateStreaming = (...args) => instantiateStreaming(...args).then(capture);
}

const view = () => new DataView(memory.buffer);
const bytes = (ptr, len) => new Uint8Array(memory.buffer, ptr, len);
const decoder = new TextDecoder();
const readString = (ptr, len) => decoder.decode(bytes(ptr, len).slice());

function fullPath(dirFd, ptr, len) {
  if (dirFd !== PREOPEN_FD) return null;
  const parts = [];
  for (const p of readString(ptr, len).split("/")) {
    if (p === "" || p === ".") continue;
    if (p === "..") parts.pop();
    else parts.push(p);
  }
  return "/" + parts.join("/");
}

// 標準出力・標準エラー出力を行ごとにコンソールへ
const lines = { 1: "", 2: "" };
function writeConsole(fd, text) {
  lines[fd] += text;
  let i;
  while ((i = lines[fd].indexOf("\n")) >= 0) {
    const line = lines[fd].slice(0, i);
    lines[fd] = lines[fd].slice(i + 1);
    (fd === 2 ? console.warn : console.log)("[kernel]", line);
  }
}

function grow(file, need) {
  if (need <= file.data.length) return;
  const data = new Uint8Array(Math.max(need, file.data.length * 2, 4096));
  data.set(file.data.subarray(0, file.size));
  file.data = data;
}

function iovecs(iovs, count) {
  const v = view();
  const out = [];
  for (let i = 0; i < count; i++) {
    out.push([v.getUint32(iovs + i * 8, true), v.getUint32(iovs + i * 8 + 4, true)]);
  }
  return out;
}

function writeFilestat(ptr, filetype, size) {
  const v = view();
  for (let i = 0; i < 64; i += 8) v.setBigUint64(ptr + i, 0n, true);
  v.setUint8(ptr + 16, filetype);
  v.setBigUint64(ptr + 24, 1n, true);
  v.setBigUint64(ptr + 32, BigInt(size), true);
}

export function fd_write(fd, iovs, count, nwritten) {
  let total = 0;
  if (fd === 1 || fd === 2) {
    for (const [ptr, len] of iovecs(iovs, count)) {
      writeConsole(fd, readString(ptr, len));
      total += len;
    }
  } else {
    const file = open.get(fd);
    if (!file) return EBADF;
    for (const [ptr, len] of iovecs(iovs, count)) {
      if (file.append) file.pos = file.size;
      grow(file, file.pos + len);
      file.data.set(bytes(ptr, len), file.pos);
      file.pos += len;
      file.size = Math.max(file.size, file.pos);
      total += len;
    }
  }
  view().setUint32(nwritten, total, true);
  return ESUCCESS;
}

export function fd_read(fd, iovs, count, nread) {
  const file = open.get(fd);
  let total = 0;
  if (file) {
    for (const [ptr, len] of iovecs(iovs, count)) {
      const n = Math.max(0, Math.min(len, file.size - file.pos));
      bytes(ptr, n).set(file.data.subarray(file.pos, file.pos + n));
      file.pos += n;
      total += n;
      if (n < len) break;
    }
  } else if (fd !== 0) {
    return EBADF;
  }
  view().setUint32(nread, total, true);
  return ESUCCESS;
}

export function fd_seek(fd, offset, whence, newOffset) {
  const file = open.get(fd);
  if (!file) return EBADF;
  const base = whence === 0 ? 0 : whence === 1 ? file.pos : file.size;
  const pos = base + Number(offset);
  if (pos < 0) return EINVAL;
  file.pos = pos;
  view().setBigUint64(newOffset, BigInt(pos), true);
  return ESUCCESS;
}

export function fd_tell(fd, offset) {
  const file = open.get(fd);
  if (!file) return EBADF;
  view().setBigUint64(offset, BigInt(file.pos), true);
  return ESUCCESS;
}

export function fd_close(fd) {
  const file = open.get(fd);
  if (!file) return EBADF;
  files.set(file.path, file.data.slice(0, file.size));
  open.delete(fd);
  return ESUCCESS;
}

export function fd_renumber(from, to) {
  const file = open.get(from);
  if (!file) return EBADF;
  open.delete(from);
  open.set(to, file);
  return ESUCCESS;
}

export function fd_sync(fd) {
  return open.has(fd) ? ESUCCESS : EBADF;
}

export function fd_fdstat_get(fd, ptr) {
  let filetype;
  if (fd <= 2) filetype = 2; // 文字デバイス
  else if (fd === PREOPEN_FD) filetype = 3; // ディレクトリ
  else if (open.has(fd)) filetype = 4; // 通常のファイル
  else return EBADF;
  const v = view();
  v.setUint8(ptr, filetype);
  v.setUint16(ptr + 2, 0, true);
  v.setBigUint64(ptr + 8, 0xffffffffffffffffn, true);
  v.setBigUint64(ptr + 16, 0xffffffffffffffffn, true);
  return ESUCCESS;
}

export function fd_fdstat_set_flags() {
  return ESUCCESS;
}

export function fd_filestat_get(fd, ptr) {
  if (fd <= 2) writeFilestat(ptr, 2, 0);
  else if (fd === PREOPEN_FD) writeFilestat(ptr, 3, 0);
  else if (open.has(fd)) writeFilestat(ptr, 4, open.get(fd).size);
  else return EBADF;
  return ESUCCESS;
}

export function fd_prestat_get(fd, ptr) {
  if (fd !== PREOPEN_FD) return EBADF;
  const v = view();
  v.setUint8(ptr, 0);
  v.setUint32(ptr + 4, 1, true);
  return ESUCCESS;
}

export function fd_prestat_dir_name(fd, ptr, len) {
  if (fd !== PREOPEN_FD) return EBADF;
  if (len >= 1) bytes(ptr, 1)[0] = 0x2f; // "/"
  return ESUCCESS;
}

export function path_open(dirFd, _dirflags, ptr, len, oflags, _rights, _inherit, fdflags, fdOut) {
  const path = fullPath(dirFd, ptr, len);
  if (path === null) return EBADF;
  const CREAT = 1, DIRECTORY = 2, TRUNC = 8;
  if (oflags & DIRECTORY) return path === "/" ? ENOSYS : ENOENT;
  let data = files.get(path);
  if (!data) {
    if (!(oflags & CREAT)) return ENOENT;
    data = new Uint8Array(0);
  }
  if (oflags & TRUNC) data = new Uint8Array(0);
  files.set(path, data);
  const fd = nextFd++;
  open.set(fd, { path, data: data.slice(), size: data.length, pos: 0, append: (fdflags & 1) !== 0 });
  view().setUint32(fdOut, fd, true);
  return ESUCCESS;
}

export function path_filestat_get(dirFd, _flags, ptr, len, out) {
  const path = fullPath(dirFd, ptr, len);
  if (path === null) return EBADF;
  if (path === "/") writeFilestat(out, 3, 0);
  else if (files.has(path)) writeFilestat(out, 4, files.get(path).length);
  else return ENOENT;
  return ESUCCESS;
}

export function path_unlink_file(dirFd, ptr, len) {
  const path = fullPath(dirFd, ptr, len);
  if (path === null) return EBADF;
  return files.delete(path) ? ESUCCESS : ENOENT;
}

export function path_create_directory() {
  return ESUCCESS;
}

export function path_remove_directory() {
  return ESUCCESS;
}

export function path_rename(oldFd, oldPtr, oldLen, newFd, newPtr, newLen) {
  const from = fullPath(oldFd, oldPtr, oldLen);
  const to = fullPath(newFd, newPtr, newLen);
  if (from === null || to === null) return EBADF;
  if (!files.has(from)) return ENOENT;
  files.set(to, files.get(from));
  files.delete(from);
  return ESUCCESS;
}

export function fd_readdir() {
  return ENOSYS;
}

function zeroCounts(countPtr, sizePtr) {
  view().setUint32(countPtr, 0, true);
  view().setUint32(sizePtr, 0, true);
  return ESUCCESS;
}
export const environ_sizes_get = zeroCounts;
export const args_sizes_get = zeroCounts;
export function environ_get() {
  return ESUCCESS;
}
export function args_get() {
  return ESUCCESS;
}

export function clock_time_get(id, _precision, out) {
  const ms = id === 0 ? Date.now() : performance.now();
  view().setBigUint64(out, BigInt(Math.round(ms * 1e6)), true);
  return ESUCCESS;
}

export function clock_res_get(_id, out) {
  view().setBigUint64(out, 1000n, true);
  return ESUCCESS;
}

export function random_get(ptr, len) {
  for (let i = 0; i < len; i += 65536) {
    crypto.getRandomValues(bytes(ptr + i, Math.min(65536, len - i)));
  }
  return ESUCCESS;
}

export function sched_yield() {
  return ESUCCESS;
}

export function poll_oneoff() {
  return ENOSYS;
}

export function proc_exit(code) {
  throw new Error(`形状カーネルが終了しました (コード ${code})`);
}
