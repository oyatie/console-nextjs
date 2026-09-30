"use client";

import React from "react";
import { ObjectLink } from "./ObjectLink";

interface DynamicTextProps {
  text: string;
  className?: string;
}

// Regex to match:
// 1. [CODE] or #CODE or plain CODE where CODE is AP-*, WO-*, AT-*, EMP-*, PS-*, C-*, PR-*
// 2. @Name mentions
const ENTITY_REGEX = /(\[(?:AP|WO|AT|EMP|PS|C|PR)-[\w-]+\]|#(?:AP|WO|AT|EMP|PS|C|PR)-[\w-]+|(?:AP|WO|AT|EMP|PS|C|PR)-[\w-]+|@[가-힣\w]+)/g;

export function DynamicText({ text, className }: DynamicTextProps) {
  if (!text) return null;

  const parts = text.split(ENTITY_REGEX);

  return (
    <span className={className}>
      {parts.map((part, idx) => {
        if (!part) return null;

        // Check if it's an entity reference
        const cleanPart = part.replace(/^[\[#]/, "").replace(/\]$/, "");

        if (
          cleanPart.startsWith("AP-") ||
          cleanPart.startsWith("WO-") ||
          cleanPart.startsWith("AT-") ||
          cleanPart.startsWith("EMP-") ||
          cleanPart.startsWith("PS-") ||
          cleanPart.startsWith("C-") ||
          cleanPart.startsWith("PR-")
        ) {
          return <ObjectLink key={idx} code={cleanPart} className="mx-0.5 my-0.5" />;
        }

        // Check if it's a person mention @Name
        if (part.startsWith("@")) {
          return (
            <span
              key={idx}
              className="inline-flex items-center px-1.5 py-0.5 rounded-[4px] bg-amber-100/70 text-amber-950 font-semibold text-[11px] font-sans mx-0.5"
            >
              {part}
            </span>
          );
        }

        // Regular text
        return <React.Fragment key={idx}>{part}</React.Fragment>;
      })}
    </span>
  );
}
