// 노드 북마크 켜고 끄기(목업 docs/mockups/bookmarks.html). 카드 머리, 오른쪽 칸 머리, 트리 줄, 사이드바 북마크 영역이 쓴다.
// 그래프(Graph.tsx)가 이 파일을 쓰므로 이 파일은 그래프를 쓰지 않는다.
import { Bookmark } from 'lucide-react';
import { useState } from 'react';
import { bookmarkNode, unbookmarkNode } from './api';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';

/** 노드 하나의 북마크 켜고 끄기. `label`은 누르면 일어날 일이다. */
export function bookmarkToggle(view: NodeView): { label: string; pressed: boolean; run: () => Promise<null> } {
  const pressed = view.bookmark !== null;
  return pressed
    ? { label: i18next.t('bookmark.remove'), pressed, run: () => unbookmarkNode(view.id) }
    : { label: i18next.t('bookmark.add'), pressed, run: () => bookmarkNode(view.id) };
}

/** 카드 머리·오른쪽 칸 머리의 북마크 단추. 늘 보인다. 거절되면 단추가 빨갛게 바뀌고 툴팁이 이유를 보인다. */
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
