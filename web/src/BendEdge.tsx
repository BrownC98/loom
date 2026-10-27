import { BaseEdge, getSmoothStepPath, type Edge, type EdgeProps } from '@xyflow/react';

export type BendEdgeType = Edge<{ center: { x?: number; y?: number } }, 'bend'>;

export function BendEdge(props: EdgeProps<BendEdgeType>) {
  const [path, labelX, labelY] = getSmoothStepPath({
    sourceX: props.sourceX,
    sourceY: props.sourceY,
    sourcePosition: props.sourcePosition,
    targetX: props.targetX,
    targetY: props.targetY,
    targetPosition: props.targetPosition,
    offset: 0,
    centerX: props.data?.center.x,
    centerY: props.data?.center.y,
  });
  return (
    <BaseEdge
      path={path}
      labelX={labelX}
      labelY={labelY}
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
