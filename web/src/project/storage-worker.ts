// Writes take audio and frames to the origin private file system. Fast synchronous
// file handles (createSyncAccessHandle) exist only in workers, so all OPFS work
// happens here. Layout (TDD "Persistence"):
//   projects/<id>/audio/<ref>.f32, projects/<id>/frames/<ref>.f32, projects/<id>/blobs/<ref>.f32
// The folder comes from the ref prefix ('audio-', 'frames-', anything else: blobs).

export type StorageRequest =
  | { id: number; op: 'put'; project: string; ref: string; data: ArrayBuffer }
  | { id: number; op: 'get'; project: string; ref: string }
  | { id: number; op: 'del'; project: string; ref: string }
  | { id: number; op: 'list'; project: string };

export type StorageReply =
  | { id: number; ok: true; data?: ArrayBuffer | null; refs?: string[] }
  | { id: number; ok: false; error: string };

export const FOLDERS = ['audio', 'frames', 'blobs'] as const;

export function folderFor(ref: string): (typeof FOLDERS)[number] {
  if (ref.startsWith('audio-')) return 'audio';
  if (ref.startsWith('frames-')) return 'frames';
  return 'blobs';
}

/** Refs become file names, so reject anything that could leave the folder. */
export function checkName(name: string): void {
  if (!name || /[/\\]/.test(name) || name === '.' || name === '..' || name.includes('\0')) {
    throw new RangeError(`bad name: ${JSON.stringify(name)}`);
  }
}

interface SyncHandle {
  read(buf: ArrayBufferView, opts?: { at: number }): number;
  write(buf: ArrayBufferView, opts?: { at: number }): number;
  truncate(size: number): void;
  getSize(): number;
  flush(): void;
  close(): void;
}
interface FileHandleSync extends FileSystemFileHandle {
  createSyncAccessHandle(): Promise<SyncHandle>;
}

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<StorageRequest>) => void) | null;
  postMessage(msg: StorageReply, transfer?: Transferable[]): void;
};

async function dir(project: string, folder: string, create: boolean): Promise<FileSystemDirectoryHandle> {
  checkName(project);
  let d = await navigator.storage.getDirectory();
  for (const part of ['projects', project, folder]) d = await d.getDirectoryHandle(part, { create });
  return d;
}

const isMissing = (e: unknown) => e instanceof DOMException && (e.name === 'NotFoundError' || e.name === 'TypeMismatchError');

async function put(project: string, ref: string, data: ArrayBuffer): Promise<void> {
  checkName(ref);
  const d = await dir(project, folderFor(ref), true);
  const fh = (await d.getFileHandle(`${ref}.f32`, { create: true })) as FileHandleSync;
  const h = await fh.createSyncAccessHandle();
  try {
    h.truncate(0);
    h.write(new Uint8Array(data), { at: 0 });
    h.flush();
  } finally {
    h.close();
  }
}

async function get(project: string, ref: string): Promise<ArrayBuffer | null> {
  checkName(ref);
  try {
    const d = await dir(project, folderFor(ref), false);
    const fh = (await d.getFileHandle(`${ref}.f32`)) as FileHandleSync;
    const h = await fh.createSyncAccessHandle();
    try {
      const out = new Uint8Array(h.getSize());
      h.read(out, { at: 0 });
      return out.buffer;
    } finally {
      h.close();
    }
  } catch (e) {
    if (isMissing(e)) return null;
    throw e;
  }
}

async function del(project: string, ref: string): Promise<void> {
  checkName(ref);
  try {
    const d = await dir(project, folderFor(ref), false);
    await d.removeEntry(`${ref}.f32`);
  } catch (e) {
    if (!isMissing(e)) throw e;
  }
}

async function list(project: string): Promise<string[]> {
  const refs: string[] = [];
  for (const folder of FOLDERS) {
    let d: FileSystemDirectoryHandle;
    try {
      d = await dir(project, folder, false);
    } catch (e) {
      if (isMissing(e)) continue;
      throw e;
    }
    for await (const [name, h] of d.entries()) {
      if (h.kind === 'file' && name.endsWith('.f32')) refs.push(name.slice(0, -4));
    }
  }
  return refs;
}

// One request at a time: two sync handles on one file would throw, and ordering
// (put then get of the same ref) must hold.
let chain: Promise<void> = Promise.resolve();

scope.onmessage = (e) => {
  const req = e.data;
  chain = chain.then(async () => {
    try {
      switch (req.op) {
        case 'put':
          await put(req.project, req.ref, req.data);
          scope.postMessage({ id: req.id, ok: true });
          break;
        case 'get': {
          const data = await get(req.project, req.ref);
          scope.postMessage({ id: req.id, ok: true, data }, data ? [data] : []);
          break;
        }
        case 'del':
          await del(req.project, req.ref);
          scope.postMessage({ id: req.id, ok: true });
          break;
        case 'list':
          scope.postMessage({ id: req.id, ok: true, refs: await list(req.project) });
          break;
      }
    } catch (err) {
      scope.postMessage({ id: req.id, ok: false, error: err instanceof Error ? err.message : String(err) });
    }
  });
};
