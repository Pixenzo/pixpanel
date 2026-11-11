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
  /** Üzerinde gezinilen noktanın sırası; verilirse o noktada dikey bir imleç çizilir. */
  cursor?: number | null;
  /** Fare ya da parmak izin üzerinde gezdikçe en yakın noktanın sırasını bildirir. */
  onHover?: (index: number | null) => void;
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

export function Trace({ values, secondary, max, color = "--accent", slots, label, cursor, onHover }: TraceProps) {
  const gradientId = useId();
  const peak = Math.max(...values, ...(secondary ?? []), 0);
  // Tavan, tepe değerin biraz üstünde tutulur ki iz üst kenara yapışmasın.
  const ceiling = max ?? Math.max(peak * 1.25, 1);
  const line = buildPath(values, ceiling, slots);
  const step = WIDTH / Math.max(slots - 1, 1);
  const startX = (slots - values.length) * step;
  const area = line ? `${line} L${WIDTH},${HEIGHT} L${startX.toFixed(1)},${HEIGHT} Z` : "";
  const cursorX = cursor == null ? null : startX + cursor * step;
  const cursorY = cursor == null || values[cursor] === undefined ? null : HEIGHT - (Math.min(values[cursor], ceiling) / ceiling) * (HEIGHT - 4) - 2;

  /** İmlecin yatay konumundan en yakın noktanın sırasını bulur. */
  function locate(clientX: number, element: SVGSVGElement) {
    if (!onHover || values.length === 0) return;
    const box = element.getBoundingClientRect();
    const x = ((clientX - box.left) / box.width) * WIDTH;
    const index = Math.round((x - startX) / step);
    onHover(Math.max(0, Math.min(values.length - 1, index)));
  }

  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      preserveAspectRatio="none"
      className={onHover ? "h-16 w-full touch-none" : "h-16 w-full"}
      style={{ color: `var(${color})` }}
      onPointerMove={onHover ? (event) => locate(event.clientX, event.currentTarget) : undefined}
      onPointerDown={onHover ? (event) => locate(event.clientX, event.currentTarget) : undefined}
      onPointerLeave={onHover ? () => onHover(null) : undefined}
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
      {cursorX !== null && <line x1={cursorX} x2={cursorX} y1="0" y2={HEIGHT} stroke="var(--ink-3)" strokeWidth="1" vectorEffect="non-scaling-stroke" />}
      {cursorX !== null && cursorY !== null && (
        // Nokta, çizgiyle kesiştiği yerde okunaklı kalsın diye yüzey renginde bir halka taşır.
        <ellipse cx={cursorX} cy={cursorY} rx={(4 * WIDTH) / 900} ry="4" fill="currentColor" stroke="var(--surface)" strokeWidth="2" vectorEffect="non-scaling-stroke" />
      )}
    </svg>
  );
}
