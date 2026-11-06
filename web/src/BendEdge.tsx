import { BaseEdge, type Edge, type EdgeProps } from '@xyflow/react';
import { roundedPath } from './edgePath';
import type { BendCenter } from './layout';

export type BendEdgeType = Edge<{ center: BendCenter }, 'bend'>;

export function BendEdge(props: EdgeProps<BendEdgeType>) {
  const { sourceX, sourceY, targetX, targetY } = props;
  const center = props.data!.center;
  const bend =
    'y' in center
      ? { points: [{ x: sourceX, y: center.y }, { x: targetX, y: center.y }], label: { x: (sourceX + targetX) / 2, y: center.y } }
      : { points: [{ x: center.x, y: sourceY }, { x: center.x, y: targetY }], label: { x: center.x, y: (sourceY + targetY) / 2 } };
  return (
    <BaseEdge
      path={roundedPath([{ x: sourceX, y: sourceY }, ...bend.points, { x: targetX, y: targetY }])}
      labelX={bend.label.x}
      labelY={bend.label.y}
      label={props.label}
      labelStyle={props.labelStyle}
      labelShowBg={props.labelShowBg}
      labelBgStyle={props.labelBgStyle}
      labelBgPadding={props.labelBgPadding}
      labelBgBorderRadius={props.labelBgBorderRadius}
      style={props.style}
      markerStart={props.markerStart}
      markerEnd={props.markerEnd}
      interactionWidth={props.interactionWidth}
    />
  );
}
