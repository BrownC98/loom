import { Bookmark } from 'lucide-react';
import { useState } from 'react';
import { bookmarkNode, unbookmarkNode } from './api';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';

export function bookmarkToggle(view: NodeView): { label: string; pressed: boolean; run: () => Promise<null> } {
  const pressed = view.bookmark !== null;
  return pressed
    ? { label: i18next.t('bookmark.remove'), pressed, run: () => unbookmarkNode(view.id) }
    : { label: i18next.t('bookmark.add'), pressed, run: () => bookmarkNode(view.id) };
}

export function BookmarkButton(props: { view: NodeView }) {
  const toggle = bookmarkToggle(props.view);
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <button
      type="button"
      className="btn ghost sm icon nodrag bookmark"
      aria-label={toggle.label}
      aria-pressed={toggle.pressed}
      title={refusal ?? toggle.label}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        toggle.run().catch((err: Error) => setRefusal(err.message));
      }}
    >
      <Bookmark />
    </button>
  );
}
