export type SaveName = (name: string) => Promise<unknown>;

export function editEnd() {
  let ended = false;
  return {
    cancel(close: () => void) {
      ended = true;
      close();
    },
    save(value: string, name: string, save: SaveName, close: () => void, fail: (message: string) => void): boolean {
      if (ended) return false;
      ended = true;
      if (value.trim() === name) {
        close();
        return false;
      }
      save(value).then(close, (e: Error) => {
        ended = false;
        fail(e.message);
      });
      return true;
    },
  };
}
