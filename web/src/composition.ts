export const IME_KEY_CODE = 229;

export type Composition = {
  afterText: (run: () => void) => boolean;
  dispose: () => void;
};

export function watchComposition(target: EventTarget, onComposing?: (on: boolean) => void): Composition {
  let composing = false;
  let settling = 0;
  let disposed = false;
  const settle = () => {
    settling += 1;
    setTimeout(() => (settling -= 1));
  };
  const start = () => {
    composing = true;
    onComposing?.(true);
  };
  const end = () => {
    composing = false;
    onComposing?.(false);
    settle();
  };
  target.addEventListener('compositionstart', start);
  target.addEventListener('compositionupdate', settle);
  target.addEventListener('compositionend', end);
  return {
    afterText: (run) => {
      if (!composing && settling === 0) return false;
      setTimeout(() => disposed || run());
      return true;
    },
    dispose: () => {
      disposed = true;
      if (composing) onComposing?.(false);
      composing = false;
      target.removeEventListener('compositionstart', start);
      target.removeEventListener('compositionupdate', settle);
      target.removeEventListener('compositionend', end);
    },
  };
}
