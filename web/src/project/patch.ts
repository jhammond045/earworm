// Immutable patching with inverses (TDD "Commands and undo"). A command turns into a list
// of Op; applying them gives the new document plus the ops that undo them.

export type Path = (string | number)[];
/** set: replace (or add) the value at path; value undefined deletes an object key.
 *  ins: insert value into the array at path (last element is the index). rm: remove the array element at path. */
export type Op = { op: 'set'; path: Path; value: unknown } | { op: 'ins'; path: Path; value: unknown } | { op: 'rm'; path: Path };

type Container = Record<string | number, unknown> | unknown[];

const bad = (msg: string, path: Path): never => { throw new RangeError(`patch: ${msg} at [${path.join('/')}]`); };

function isContainer(v: unknown): v is Container {
  return typeof v === 'object' && v !== null;
}

function index(c: unknown[], key: string | number, max: number, path: Path): number {
  if (typeof key !== 'number' || !Number.isInteger(key) || key < 0 || key > max) bad(`bad array index ${String(key)}`, path);
  return key as number;
}

/** Walk to the parent of the last path element, copying each container on the way down
 *  only if `leaf` changed something below it. */
function modify(node: unknown, path: Path, i: number, leaf: (c: Container, key: string | number) => Container): Container {
  if (!isContainer(node)) return bad('not an object or array', path.slice(0, i));
  if (i === path.length - 1) return leaf(node, path[i]);
  const key = path[i];
  if (!Object.prototype.hasOwnProperty.call(node, key)) return bad(`missing key ${String(key)}`, path.slice(0, i + 1));
  const child = (node as Record<string | number, unknown>)[key];
  const next = modify(child, path, i + 1, leaf);
  if (next === child) return node;
  return withKey(node, key, next);
}

function withKey(c: Container, key: string | number, value: unknown): Container {
  if (Array.isArray(c)) {
    const copy = c.slice();
    copy[key as number] = value;
    return copy;
  }
  return { ...c, [key]: value };
}

/** Apply ops in order to an immutable value with structural sharing (untouched subtrees keep identity;
 *  every object/array on a changed path is a new object). Returns the new value and the inverse ops
 *  (applying `inverse` to the result gives back a deep-equal original). Throws on a bad path. Never mutates input. */
export function applyOps<T>(doc: T, ops: Op[]): { doc: T; inverse: Op[] } {
  let cur: unknown = doc;
  const inverse: Op[] = [];
  for (const op of ops) {
    const path = op.path;
    if (path.length === 0) {
      if (op.op !== 'set') bad('ins/rm need a path', path);
      else if (op.value !== cur) {
        inverse.push({ op: 'set', path: [], value: cur });
        cur = op.value;
      }
      continue;
    }
    const parentPath = path.slice(0, -1);
    const found: { undo: Op | null } = { undo: null };
    cur = modify(cur, path, 0, (c, key) => {
      if (op.op === 'set') {
        if (Array.isArray(c)) {
          const k = index(c, key, c.length - 1, path);
          if (op.value === undefined) return bad('cannot set undefined in an array', path);
          if (Object.is(c[k], op.value)) return c;
          found.undo = { op: 'set', path, value: c[k] };
          return withKey(c, k, op.value);
        }
        const had = Object.prototype.hasOwnProperty.call(c, key);
        if (op.value === undefined) {
          if (!had) return c;
          found.undo = { op: 'set', path, value: c[key] };
          const copy = { ...c };
          delete copy[key];
          return copy;
        }
        if (had && Object.is(c[key], op.value)) return c;
        found.undo = { op: 'set', path, value: had ? c[key] : undefined };
        return withKey(c, key, op.value);
      }
      if (!Array.isArray(c)) return bad('ins/rm target is not an array', parentPath);
      if (op.op === 'ins') {
        const k = index(c, key, c.length, path);
        const copy = c.slice();
        copy.splice(k, 0, op.value);
        found.undo = { op: 'rm', path };
        return copy;
      }
      const k = index(c, key, c.length - 1, path);
      const copy = c.slice();
      const [old] = copy.splice(k, 1);
      found.undo = { op: 'ins', path, value: old };
      return copy;
    });
    if (found.undo) inverse.push(found.undo);
  }
  return { doc: cur as T, inverse: inverse.reverse() };
}
