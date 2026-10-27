import type { CSSProperties } from 'react';
import { BaseEdge, type Edge, type EdgeProps } from '@xyflow/react';
import type { HopRoute } from './hopLanes';

const HOP_LIFT_MIN = 40;
const CORNER = 5;
const HIT_WIDTH = 6;
const LABEL_FONT = '600 12px';
const LABEL_PAD = { x: 8, y: 2 };
const LABEL_LINE = 17;
let measure: CanvasRenderingContext2D | null = null;

export function hopLabelSize(text: string): { width: number; height: number } {
  measure ??= document.createElement('canvas').getContext('2d');
  if (measure) measure.font = `${LABEL_FONT} ${getComputedStyle(document.body).fontFamily}`;
  const width = measure ? measure.measureText(text).width : text.length * 12;
  return { width: width + 2 * LABEL_PAD.x, height: LABEL_LINE + 2 * LABEL_PAD.y };
}

export type HopEdgeType = Edge<{ route: HopRoute | null; title: string; color: string }, 'hop'>;

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
  const turned = route?.turned && route.label && typeof props.label === 'string' ? { at: route.label.at, text: props.label, size: hopLabelSize(props.label) } : null;
  return (
    <g style={{ '--hop': props.data?.color } as CSSProperties}>
      <title>{props.data?.title}</title>
      <BaseEdge
        id={props.id}
        path={path}
        markerStart={props.markerStart}
        markerEnd={props.markerEnd}
        interactionWidth={HIT_WIDTH}
        label={turned ? undefined : props.label}
        labelX={route?.label ? route.label.at.x : (sourceX + targetX) / 2}
        labelY={route?.label ? route.label.at.y : (sourceY + 6 * top + targetY) / 8}
        labelBgPadding={[LABEL_PAD.x, LABEL_PAD.y]}
        labelBgBorderRadius={props.labelBgBorderRadius}
      />
      {turned && (
        <g transform={`translate(${turned.at.x} ${turned.at.y}) rotate(-90)`} pointerEvents="visiblePainted">
          <rect className="react-flow__edge-textbg" x={-turned.size.width / 2} y={-turned.size.height / 2} width={turned.size.width} height={turned.size.height} rx={props.labelBgBorderRadius} ry={props.labelBgBorderRadius} />
          <text className="react-flow__edge-text" textAnchor="middle" dominantBaseline="central">
            {turned.text}
          </text>
        </g>
      )}
    </g>
  );
}
