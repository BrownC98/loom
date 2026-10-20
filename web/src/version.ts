// 제품 버전. `npm run build`가 scripts/version.sh의 값을 넣는다. 개발 서버에서는 비어 있다.
export const VERSION: string = import.meta.env.VITE_LOOM_VERSION ?? 'dev';
