/// <reference types="vite/client" />

declare module '*?worker' {
  const WorkerConstructor: {
    new (options?: WorkerOptions): Worker
  }
  export default WorkerConstructor
}

declare module 'node:fs' {
  export function readFileSync(path: URL, encoding: 'utf8'): string
}
