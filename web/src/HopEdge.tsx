import { BaseEdge, type Edge, type EdgeProps } from '@xyflow/react';
import type { HopRoute } from './hopLanes';

const HOP_LIFT_MIN = 40;
const CORNER = 5;

export type HopEdgeType = Edge<{ route: HopRoute | null }, 'hop'>;

function lanePath(points: HopRoute['points']): string {
  const toward = (from: number, to: number) => Math.sign(to - from) * Math.min(CORNER, Math.abs(to - from) / 2);
  const [first, ...rest] = points;
  let d = `M ${first.x} ${first.y}`;
  rest.forEach((p, i) => {
    const prev = points[i];
    const next = rest[i + 1];
    if (!next) {
      d += ` L ${p.x} ${p.y}`;
      return;
    }
    d += ` L ${p.x - toward(prev.x, p.x)} ${p.y - toward(prev.y, p.y)} Q ${p.x} ${p.y} ${p.x + toward(p.x, next.x)} ${p.y + toward(p.y, next.y)}`;
  });
  return d;
}

export function HopEdge(props: EdgeProps<HopEdgeType>) {
  const route = props.data?.route ?? null;
  const { sourceX, sourceY, targetX, targetY } = props;
  const top = Math.min(sourceY, targetY) - Math.max(HOP_LIFT_MIN, Math.abs(targetX - sourceX) / 4);
  const path = route ? lanePath(route.points) : `M ${sourceX} ${sourceY} C ${sourceX} ${top}, ${targetX} ${top}, ${targetX} ${targetY}`;
  return (
    <BaseEdge
      id={props.id}
      path={path}
      markerStart={props.markerStart}
      markerEnd={props.markerEnd}
      label={props.label}
      labelX={route ? route.label.x : (sourceX + targetX) / 2}
      labelY={route ? route.label.y : (sourceY + 6 * top + targetY) / 8}
      labelBgPadding={props.labelBgPadding}
      labelBgBorderRadius={props.labelBgBorderRadius}
    />
  );
}
