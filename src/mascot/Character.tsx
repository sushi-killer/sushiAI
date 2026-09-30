import type { Mood } from "./mood";

// Brand colours stay literal: this is art, like public/sushi.svg.
const CREAM = "#f1ead8";
const INK = "#1c241d";
const SALMON = "#ed8e74";
const EDGE = "#a0b69b";
const NORI = "#526b56";
const SALMON_EDGE = "#da785f";
const CUCUMBER = "#789365";
const SPARK = "#d6b57a";
const SWEAT = "#8fa7c9";

type Arm = { d: string; x: number; y: number };
type Pose = { left: Arm; right: Arm };

const DOWN: Pose = {
  left: { d: "M2.6 15.2q-1.6.9-2 2.6", x: 0.6, y: 17.9 },
  right: { d: "M21.4 15.2q1.6.9 2 2.6", x: 23.4, y: 17.9 },
};
const WAVE_LEFT: Arm = { d: "M2.6 14.2q-1.8-1.6-1.6-4.2", x: 1, y: 9.6 };
const POSES: Record<Mood, Pose> = {
  idle: DOWN,
  listening: DOWN,
  failed: DOWN,
  "needs-you": { left: WAVE_LEFT, right: DOWN.right },
  done: {
    left: WAVE_LEFT,
    right: { d: "M21.4 14.2q1.8-1.6 1.6-4.2", x: 23, y: 9.6 },
  },
  working: {
    left: { d: "M2.6 15.6q-.6 1.6.6 2.6", x: 3.4, y: 18.4 },
    right: { d: "M21.4 15.6q.6 1.6-.6 2.6", x: 20.6, y: 18.4 },
  },
};

function ArmShape({ arm, side }: { arm: Arm; side: "left" | "right" }) {
  return (
    <g className={`arm arm-${side}`}>
      <path
        fill="none"
        stroke={EDGE}
        strokeWidth="1.5"
        strokeLinecap="round"
        d={arm.d}
      />
      <circle
        cx={arm.x}
        cy={arm.y}
        r="1"
        fill={CREAM}
        stroke={EDGE}
        strokeWidth=".5"
      />
    </g>
  );
}

function Eyes({ px, py, ry }: { px: number; py: number; ry: number }) {
  return (
    <>
      {[9, 15].map((cx) => (
        <g key={cx}>
          <ellipse cx={cx} cy="17.3" rx=".95" ry={ry} fill={CREAM} />
          <circle cx={cx + px} cy={17.4 + py} r=".5" fill={INK} />
        </g>
      ))}
    </>
  );
}

function Cheeks() {
  return (
    <g className="cheeks">
      {[7.4, 16.6].map((cx) => (
        <ellipse
          key={cx}
          cx={cx}
          cy="18.9"
          rx=".75"
          ry=".4"
          fill={SALMON}
          opacity=".7"
        />
      ))}
    </g>
  );
}

function Line({ d, w = 0.6 }: { d: string; w?: number }) {
  return (
    <path
      d={d}
      fill="none"
      stroke={CREAM}
      strokeWidth={w}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  );
}

function Face({ mood }: { mood: Mood }) {
  switch (mood) {
    case "working":
      return (
        <>
          <g className="eyes">
            <Eyes px={0.25} py={0.35} ry={0.8} />
          </g>
          <Line d="M8 15.9l1.9.3M16 15.9l-1.9.3" w={0.45} />
          <g className="mouth">
            <Line d="M11.3 19.4h1.4" />
          </g>
        </>
      );
    case "needs-you":
      return (
        <>
          <g className="eyes">
            <Eyes px={0} py={-0.35} ry={1.25} />
          </g>
          <Cheeks />
          <g className="mouth">
            <ellipse
              cx="12"
              cy="19.5"
              rx=".6"
              ry=".7"
              fill={INK}
              stroke={CREAM}
              strokeWidth=".4"
            />
          </g>
        </>
      );
    case "listening":
      return (
        <>
          <g className="eyes">
            <Eyes px={0.35} py={-0.35} ry={1.1} />
          </g>
          <Cheeks />
          <g className="mouth">
            <Line d="M11.2 19.2q.8.5 1.6 0" />
          </g>
        </>
      );
    case "done":
      return (
        <>
          <g className="eyes">
            <Line d="M8.1 17.6l.9-.8.9.8M14.1 17.6l.9-.8.9.8" />
          </g>
          <Cheeks />
          <g className="mouth">
            <path
              d="M10.7 18.7h2.6a1.3 1.3 0 0 1-2.6 0Z"
              fill={INK}
              stroke={CREAM}
              strokeWidth=".4"
            />
          </g>
        </>
      );
    case "failed":
      return (
        <>
          <g className="eyes">
            <Eyes px={0} py={0.2} ry={1} />
          </g>
          <Line d="M8 15.7l1.8.5M16 15.7l-1.8.5" w={0.45} />
          <g className="mouth">
            <Line d="M10.8 19.6q.6-.5 1.2 0t1.2 0" />
          </g>
        </>
      );
    default:
      return (
        <>
          <g className="eyes">
            <Eyes px={0} py={0} ry={1.05} />
          </g>
          <Cheeks />
          <g className="mouth">
            <Line d="M11 19.1q1 .9 2 0" />
          </g>
        </>
      );
  }
}

function Fx({ mood }: { mood: Mood }) {
  if (mood === "working")
    return (
      <g className="fx">
        <circle className="dot dot-1" cx="21.2" cy="4" r=".55" fill={EDGE} />
        <circle
          className="dot dot-2"
          cx="22.8"
          cy="4"
          r=".55"
          fill={EDGE}
          opacity=".7"
        />
        <circle
          className="dot dot-3"
          cx="24.4"
          cy="4"
          r=".55"
          fill={EDGE}
          opacity=".4"
        />
      </g>
    );
  if (mood === "done")
    return (
      <g className="fx">
        <path
          className="sparkles"
          fill={SPARK}
          d="M4 2.5l.35.9.9.35-.9.35-.35.9-.35-.9-.9-.35.9-.35ZM20.5 1l.3.75.75.3-.75.3-.3.75-.3-.75-.75-.3.75-.3ZM23.5 6l.25.6.6.25-.6.25-.25.6-.25-.6-.6-.25.6-.25Z"
        />
      </g>
    );
  if (mood === "failed")
    return (
      <g className="fx">
        <path
          className="sweat"
          fill={SWEAT}
          d="M19.6 10.2c.5.9.9 1.4.9 1.9a.9.9 0 0 1-1.8 0c0-.5.4-1 .9-1.9Z"
        />
      </g>
    );
  return <g className="fx" />;
}

/** The mascot's animated character; each part is its own group so CSS can
 * move it on its own. */
export function Character({ mood }: { mood: Mood }) {
  const pose = POSES[mood];
  return (
    <svg
      className={`sushi ${mood}`}
      viewBox="0 0 28 28"
      width="64"
      height="64"
      aria-hidden="true"
    >
      <g transform="translate(2 1.5)">
        <g className="body">
          <path
            fill={NORI}
            stroke={EDGE}
            strokeWidth="1.45"
            strokeLinejoin="round"
            d="M2 9v6c0 1.1.6 2.2 1.8 3l3.9 2.7c2.4 1.7 6.3 1.7 8.7 0l3.9-2.7c1.2-.8 1.8-1.9 1.8-3V9"
          />
          <path
            fill={CREAM}
            stroke={CREAM}
            strokeWidth="1.45"
            strokeLinejoin="round"
            d="M16.4 3.3a8.23 8.23 0 0 0-8.8 0L3.8 6c-2.4 1.7-2.4 4.4 0 6.1l3.9 2.7c2.4 1.7 6.3 1.7 8.7 0l3.9-2.7c2.4-1.7 2.4-4.4 0-6.1Z"
          />
          <path
            fill={SALMON}
            stroke={SALMON_EDGE}
            strokeWidth="1.45"
            strokeLinejoin="round"
            d="M7.7 10.1c-.9-.6-.9-1.6 0-2.2l2.7-1.8c.9-.6 2.4-.6 3.3 0l2.7 1.8c.9.6.9 1.6 0 2.2l-2.7 1.8c-.9.6-2.4.6-3.3 0Z"
          />
          <path
            fill="none"
            stroke={CUCUMBER}
            strokeWidth="1.7"
            strokeLinecap="round"
            d="M15 11c-2-3-5-2-6 0"
          />
          <ArmShape arm={pose.left} side="left" />
          <ArmShape arm={pose.right} side="right" />
          <Face mood={mood} />
        </g>
        <Fx mood={mood} />
      </g>
    </svg>
  );
}
