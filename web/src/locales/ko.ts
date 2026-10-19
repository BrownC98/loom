// 한국어 문구. 키는 en.ts와 같아야 한다(타입이 막는다).
import type en from './en';

const ko: typeof en = {
  languageName: { en: 'English', ko: '한국어' },
  settings: {
    language: '언어',
    languageSystem: '시스템',
    languageHint: '「시스템」은 브라우저 언어를 따른다. 한국어가 아니면 영어로 보인다.',
    rescan: '다시 찾기',
    versionWarning: 'app-server {{installed}} · 고정 {{pinned}} · loom 명령마다 승인을 묻는다',
  },
  terminal: {
    gone: '데몬이 다시 시작해 이 터미널은 없다.',
  },
  problem: {
    refused: {
      emptyName: '이름을 적는다.',
      nameTaken: '{{value}}: 부모나 형제가 이미 이 이름을 쓴다.',
      tooDeep: '중첩이 너무 깊다. 자식을 더 만들지 않는다.',
      briefTooLong: '본문이 {{value}}바이트다. 줄이고, 긴 자료는 파일 경로를 적는다.',
      summaryTooLong: '요약이 {{value}}바이트다. 줄이거나 `loom done --file <경로>`를 쓴다.',
      mailboxFull: '{{value}}의 메일함이 찼다. 받는 노드가 `loom wait`로 읽은 뒤 다시 보낸다.',
      notRelated: '{{value}}: 내 부모나 자식이 아니다. 메시지는 부모와 자식 사이에만 보낸다.',
      ended: '{{value}}: 노드 프로세스가 끝났다.',
      alreadyDone: '이 노드는 이미 done이다.',
      heldFull: '부모가 없어 보관한 메시지가 너무 많다. 새 부모가 생긴 뒤 다시 보낸다.',
      cycle: '자기 자신이나 자기 자손을 부모로 정하지 않는다.',
      notFound: '{{value}}: 그런 이름의 노드가 없다.',
      ambiguous: '{{value}}: 이 이름의 노드가 둘 이상이다. 화면에서 부모를 바꾼다.',
      launch: '노드를 띄우지 못했다: {{value}}',
      db: 'DB 오류: {{value}}',
    },
    launch: {
      unknownAgent: '{{value}}: 카탈로그에 없는 에이전트다.',
      notInstalled: '{{agent}}: 데몬 PATH에서 실행 파일(`{{command}}`)을 찾지 못했다. 설치한 뒤 설정에서 [다시 찾기]를 누른다.',
      briefTooLong: '첫 지시문이 {{len}}바이트다. {{limit}}바이트 이하로 줄이고, 긴 자료는 파일 경로를 적는다.',
      server: '`{{program}}`를 띄우지 못했다: {{cause}}',
      noServer: '{{value}}: 노드가 붙을 서버를 띄우지 못했다.',
      noBrief: '{{value}}: 첫 지시문을 받지 않는다. 지시문을 비우고 만든 뒤 터미널에 입력한다.',
    },
    noLogin: '없는 로그인 창이다.',
    internal: '내부 오류: {{value}}',
  },
  usageProblem: {
    cliFailed: '`{{command}}`를 실행하지 못했다: {{detail}}',
    cliFormat: '`{{value}}`가 알 수 없는 형식을 냈다.',
    noReply: '`{{value}}`가 답하지 않았다.',
    loginNotSupported: '이 로그인 방식(API 키 등)은 사용량을 볼 수 없다.',
    responseFormat: '사용량 응답이 알 수 없는 형식이다.',
    tokenExpired: '토큰이 만료됐다. 이 계정으로 Claude를 한 번 실행하면 갱신된다.',
    unavailable: '사용량을 지금 볼 수 없다: {{value}}',
  },
};

export default ko;
