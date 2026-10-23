import { BaseEdge, type EdgeProps } from '@xyflow/react';

const HOP_LIFT_MIN = 40;

export function HopEdge(props: EdgeProps) {
  const { sourceX, sourceY, targetX, targetY } = props;
  const top = Math.min(sourceY, targetY) - Math.max(HOP_LIFT_MIN, Math.abs(targetX - sourceX) / 4);
  const path = `M ${sourceX} ${sourceY} C ${sourceX} ${top}, ${targetX} ${top}, ${targetX} ${targetY}`;
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
