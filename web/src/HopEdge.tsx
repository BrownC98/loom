// 간선 없는 두 노드 사이 메시지를 그리는 선(목업 docs/mockups/any-message.html v1 A).
import { BaseEdge, type EdgeProps } from '@xyflow/react';

/** 곡선이 두 카드 위로 휘는 높이의 하한. 같은 줄의 가까운 두 카드도 곡선이 카드 머리를 덮지 않고 보이게 한다. */
const HOP_LIFT_MIN = 40;

/** 두 카드의 위 변을 잇고 위로 휘는 곡선. 높이는 두 점 사이 가로 거리의 1/4이다. 라벨은 곡선 가운데에 둔다. */
export function HopEdge(props: EdgeProps) {
  const { sourceX, sourceY, targetX, targetY } = props;
  const top = Math.min(sourceY, targetY) - Math.max(HOP_LIFT_MIN, Math.abs(targetX - sourceX) / 4);
  const path = `M ${sourceX} ${sourceY} C ${sourceX} ${top}, ${targetX} ${top}, ${targetX} ${targetY}`;
  // 3차 베지어의 가운데(t = 1/2)는 (P0 + 3P1 + 3P2 + P3) / 8이다.
  const labelY = (sourceY + 6 * top + targetY) / 8;
  return (
    <BaseEdge
      id={props.id}
      path={path}
      label={props.label}
      labelX={(sourceX + targetX) / 2}
      labelY={labelY}
      labelBgPadding={props.labelBgPadding}
      labelBgBorderRadius={props.labelBgBorderRadius}
    />
  );
}
