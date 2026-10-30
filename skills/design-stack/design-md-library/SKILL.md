---
name: design-md-library
description: "Use when starting a UI without a design direction, or when asked to build in a brand's style (Vercel, Apple, Airbnb, Figma, Linear)."
version: 1.0.0
author: "VoltAgent (awesome-design-md), Hermes Agent"
license: MIT
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [design, design-system, ui, style, brand, tokens]
    related_skills: [design-md, better-interface, better-ui, emil-design-eng]
---

# 디자인 스타일 라이브러리 (DESIGN.md)

실제 제품에서 추출한 디자인 시스템 스펙 모음. 디자인 방향이 정해지지 않은 UI 작업에서는 값을 임의로 발명하지 말고 여기 있는 스펙 중 하나를 골라 그대로 따른다. 출처: VoltAgent awesome-design-md(iFurySt/DESIGN.md 미러, MIT), DCInside 특이점 갤러리 "AI 디자인 퀄리티 올리는 팁"(2026-09-11) 내재화.

## When to Use

- 자체 디자인 시스템이 없는 새 UI·프로토타입 제작
- "Linear 스타일", "Vercel 느낌" 같은 브랜드 스타일 요청
- 세션마다 AI의 시각적 결정이 달라져 일관성이 무너질 때 - 스펙을 고정 기준으로 삼는다

## 사용 가능한 스타일 (references/)

| 파일 | 스타일 한 줄 |
|---|---|
| `references/vercel.md` | 흑백 정밀, Geist 폰트, 극도로 절제된 액센트 |
| `references/apple.md` | 순흑/#f5f5f7 교차 리듬, SF Pro, 단일 Apple Blue |
| `references/airbnb.md` | 따뜻한 코랄, 사진 중심, 라운드 UI |
| `references/figma.md` | 다채색, 플레이풀하면서 프로페셔널 |
| `references/linear.md` | 다크 네이티브(#08090a), Inter 510, 인디고바이올렛 단일 액센트 |

그 외 브랜드(Stripe, Notion, Spotify 등 60여 개)는 github.com/iFurySt/DESIGN.md에서 필요할 때만 내려받는다. 통째로 설치하지 않는다.

## How to Apply

1. 선택한 `references/<brand>.md`를 통째로 읽는다.
2. 색·타이포그래피·간격·반경·그림자·모션 토큰을 추출한다.
3. 새 프로젝트면 루트에 `DESIGN.md`로 복사한다 - 이후 모든 UI 생성이 이 파일을 기준으로 따르게 한다.
4. 기존 시스템(AntD 등)이 있는 프로젝트면 토큰 값을 기존 컴포넌트 시스템으로 번역해 적용한다. 두 시스템을 섞지 않는다.
5. 스타일은 반드시 하나만 고른다.

## Pitfalls

- 다크 스타일(linear)을 밝은 제품에 무리하게 적용하지 않는다 - 제품 성격과 실사용 환경이 우선한다.
- 스펙은 참고 기준이지 법이 아니다: 기존 규칙(ktasearch UI 규칙 등)이 있으면 그것이 우선한다.

## 작업 규율 (원문 글 요지, UI 작업마다 적용)

- **레이아웃 우선**: 무지성 상세페이지/대시보드가 아니라, 목적을 설명하고 "이 목적에 가장 잘 맞는 UI·레이아웃 구성"부터 고민한다.
- **일관성 목줄**: 작업 도중 반복해서 점검한다 - 패딩·레이아웃·폭 일정성, 어색한 모션, 타이포그래피 위계. 이 점검은 `better-interface`, `better-ui`, `emil-design-eng` 스킬이 수행한다.
- **레퍼런스 이미지 우선**: 생산성·공학 툴류의 성격이 분명한 제품이면 프롬프트·스킬보다 레퍼런스 이미지 몇 장이 더 효과적이다(예: Pinterest "pro desktop app ui" 검색).
- **일반론 스킬 경계**: ui-ux-pro-max류의 일반론 스킬을 따르면 오히려 AI스러운 결과물이 나온다 - 위 실무 디자이너 스킬을 우선한다.
