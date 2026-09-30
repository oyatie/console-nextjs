// index.tsx — Enterprise i18n & Glossary Context Provider and Hook
"use client";

import React, { createContext, useContext, useState, useMemo, useCallback } from "react";
import { ENTERPRISE_GLOSSARY, GlossaryEntry, t as rawT, getGlossaryEntry, searchGlossary } from "./glossary";

interface I18nContextType {
  locale: "ko" | "en";
  setLocale: (locale: "ko" | "en") => void;
  t: (key: string, params?: Record<string, string | number>) => string;
  getEntry: (key: string) => GlossaryEntry | undefined;
  search: (query: string, category?: GlossaryEntry["category"]) => GlossaryEntry[];
  allTerms: Record<string, GlossaryEntry>;
}

const I18nContext = createContext<I18nContextType>({
  locale: "ko",
  setLocale: () => {},
  t: (key, params) => rawT(key, params, "ko"),
  getEntry: getGlossaryEntry,
  search: searchGlossary,
  allTerms: ENTERPRISE_GLOSSARY,
});

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocale] = useState<"ko" | "en">("ko");

  const t = useCallback(
    (key: string, params?: Record<string, string | number>) => {
      return rawT(key, params, locale);
    },
    [locale]
  );

  const value = useMemo(
    () => ({
      locale,
      setLocale,
      t,
      getEntry: getGlossaryEntry,
      search: searchGlossary,
      allTerms: ENTERPRISE_GLOSSARY,
    }),
    [locale, t]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useGlossary() {
  return useContext(I18nContext);
}

export { ENTERPRISE_GLOSSARY, rawT as tGlobal, getGlossaryEntry, searchGlossary };
export type { GlossaryEntry };
