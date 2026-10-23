// 실측(2026-10-23): 카드를 열 때마다 xterm이 backlog 속 옛 색 질의에 다시 답해 셸 노드에 `^[]11;rgb:0a0a/…`가 글로 찍혔다.
// 색 답만 버린다. 커서 위치(`CSI 6n`) 같은 다른 답은 화면이 붙기 전부터 기다리는 프로그램이 있다(리뷰 2026-10-23: 버리면 멈췄다). 사람의 키도 같은 `onData`로 온다.
const COLOR_REPLY = /\x1b\](?:10|11|4);[^\x07\x1b]*(?:\x07|\x1b\\)/g;

/** xterm이 낸 입력에서 색 답(OSC 10·11·4)을 뺀다. */
export function withoutColorReplies(data: string): string {
  return data.replace(COLOR_REPLY, '');
}
