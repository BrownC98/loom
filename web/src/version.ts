// 제품 버전의 원천은 루트 Cargo.toml이다. Windows 빌드는 화면 빌드 전에 PATCH를 이 줄에 쓴다.
import cargo from '../../Cargo.toml?raw';

export const VERSION = /^version = "(.+)"$/m.exec(cargo)![1];
