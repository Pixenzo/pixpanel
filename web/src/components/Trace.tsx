// Zaman serisi izi: osiloskop kanalı gibi, ortak zaman ekseninde tek bir çizgi.

import { useId } from "react";

interface TraceProps {
  /** Eskiden yeniye sıralı değerler. */
  values: number[];
  /** Dikey eksenin üst sınırı; verilmezse veriye göre belirlenir. */
  max?: number;
  /** İkinci, daha silik bir seri (ör. gönderilen veri). */
  secondary?: number[];
  /** Çizgi rengi için CSS değişkeni adı. */
  color?: string;
  /** Kaç örneklik yer ayrılacağı; veri azsa iz sağa yaslanır. */
  slots: number;
  label: string;
}

const WIDTH = 600;
const HEIGHT = 64;

function buildPath(values: number[], max: number, slots: number): string {
  if (values.length === 0) return "";
  const step = WIDTH / Math.max(slots - 1, 1);
  const offset = slots - values.length;
  return values
    .map((value, index) => {
      const x = (offset + index) * step;
      const y = HEIGHT - (Math.min(value, max) / max) * (HEIGHT - 4) - 2;
      return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

export function Trace({ values, secondary, max, color = "--accent", slots, label }: TraceProps) {
  const gradientId = useId();
  const peak = Math.max(...values, ...(secondary ?? []), 0);
  // Tavan, tepe değerin biraz üstünde tutulur ki iz üst kenara yapışmasın.
  const ceiling = max ?? Math.max(peak * 1.25, 1);
  const line = buildPath(values, ceiling, slots);
  const step = WIDTH / Math.max(slots - 1, 1);
  const startX = (slots - values.length) * step;
  const area = line ? `${line} L${WIDTH},${HEIGHT} L${startX.toFixed(1)},${HEIGHT} Z` : "";

  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      preserveAspectRatio="none"
      className="h-16 w-full"
      style={{ color: `var(${color})` }}
    >
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="currentColor" stopOpacity="0.22" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </linearGradient>
      </defs>
      {[0.25, 0.5, 0.75].map((ratio) => (
        <line
          key={ratio}
          x1="0"
          x2={WIDTH}
          y1={HEIGHT * ratio}
          y2={HEIGHT * ratio}
          stroke="var(--line)"
          strokeWidth="1"
          vectorEffect="non-scaling-stroke"
        />
      ))}
      {area && <path d={area} fill={`url(#${gradientId})`} />}
      {secondary && (
        <path
          d={buildPath(secondary, ceiling, slots)}
          fill="none"
          stroke="var(--ink-3)"
          strokeWidth="1.25"
          strokeDasharray="3 3"
          vectorEffect="non-scaling-stroke"
        />
      )}
      {line && (
        <path d={line} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
      )}
    </svg>
  );
}
