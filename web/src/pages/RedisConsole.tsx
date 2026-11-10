// Redis konsolu: redis-cli benzeri komut satırı. Yanıtlar özyinelemeli yapıdan redis-cli biçiminde yazıya çevrilir.

import { useQueryClient } from "@tanstack/react-query";
import type { KeyboardEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { Button, Spinner } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import type { BrowserInfo } from "./RedisBrowser";
import { DbSelect, useDbParam } from "./RedisBrowser";

// ---- Türler ----

type Reply =
  | { kind: "status" | "string"; value: string; truncated?: boolean }
  | { kind: "integer"; value: number }
  | { kind: "nil" }
  | { kind: "error"; value: string }
  | { kind: "array"; items: Reply[]; total: number };

type Outcome = { state: "running" } | { state: "reply"; reply: Reply } | { state: "failed"; message: string };

interface Entry {
  id: number;
  db: number;
  command: string;
  outcome: Outcome;
}

interface Line {
  text: string;
  tone?: "bad" | "muted";
}

const HISTORY_LIMIT = 100;
const ENTRY_LIMIT = 500;
const EXAMPLES = ["INFO memory", "DBSIZE", "SCAN 0 COUNT 20", "TTL anahtar"];

// ---- Yanıt biçimlendirme ----

/** Yanıtı redis-cli düzeninde satırlara çevirir; iç içe dizilerde hizalama korunur. */
function replyLines(reply: Reply, top: boolean): Line[] {
  switch (reply.kind) {
    case "status":
      return [{ text: reply.value }];
    case "string": {
      // INFO gibi çok satırlı yanıtlar redis-cli'deki gibi ham yazılır.
      if (top && reply.value.includes("\n")) {
        const lines: Line[] = reply.value.split(/\r?\n/).map((text) => ({ text }));
        while (lines.length > 0 && lines[lines.length - 1]?.text === "") lines.pop();
        if (reply.truncated) lines.push({ text: t("(kısaltıldı)"), tone: "muted" });
        return lines;
      }
      return [{ text: JSON.stringify(reply.value) + (reply.truncated ? ` ${t("(kısaltıldı)")}` : "") }];
    }
    case "integer":
      return [{ text: `(integer) ${reply.value}` }];
    case "nil":
      return [{ text: "(nil)" }];
    case "error":
      return [{ text: `(error) ${reply.value}`, tone: "bad" }];
    case "array": {
      if (reply.items.length === 0) return [{ text: t("(boş dizi)"), tone: "muted" }];
      const width = String(reply.items.length).length;
      const padding = " ".repeat(width + 2);
      const out: Line[] = [];
      reply.items.forEach((item, index) => {
        replyLines(item, false).forEach((line, position) => {
          const prefix = position === 0 ? `${String(index + 1).padStart(width)}) ` : padding;
          out.push({ text: prefix + line.text, tone: line.tone });
        });
      });
      if (reply.total > reply.items.length) out.push({ text: padding + t("… {n} öğe daha", { n: reply.total - reply.items.length }), tone: "muted" });
      return out;
    }
  }
}

function lineTone(tone: Line["tone"]): string {
  if (tone === "bad") return "text-bad";
  if (tone === "muted") return "text-ink-3";
  return "text-ink";
}

function EntryView({ entry }: { entry: Entry }) {
  const outcome = entry.outcome;
  const lines: Line[] =
    outcome.state === "reply" ? replyLines(outcome.reply, true) : outcome.state === "failed" ? [{ text: `(error) ${outcome.message}`, tone: "bad" }] : [];

  return (
    <div className="mb-3">
      <div className="break-all whitespace-pre-wrap text-ink">
        <span className="text-accent">{`db${entry.db}>`}</span> {entry.command}
      </div>
      {outcome.state === "running" && <Spinner className="mt-1 size-3.5 text-ink-3" />}
      {lines.map((line, index) => (
        <div key={index} className={classNames("break-all whitespace-pre-wrap", lineTone(line.tone))}>
          {line.text}
        </div>
      ))}
    </div>
  );
}

// ---- Konsol ----

export function RedisConsole({ info }: { info: BrowserInfo }) {
  const queryClient = useQueryClient();
  const [db, setDb] = useDbParam(info.databases);

  const [entries, setEntries] = useState<Entry[]>([]);
  const [input, setInput] = useState("");
  const [running, setRunning] = useState(false);

  const inputRef = useRef<HTMLInputElement>(null);
  const screenRef = useRef<HTMLDivElement>(null);
  const nextId = useRef(1);
  // Komut geçmişi: eskiden yeniye; işaretçi sondan sayılır (-1: yeni komut yazılıyor).
  const recall = useRef<string[]>([]);
  const pointer = useRef(-1);
  const draft = useRef("");

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Yeni çıktı geldikçe en alta kaydırılır.
  useEffect(() => {
    const screen = screenRef.current;
    if (screen) screen.scrollTop = screen.scrollHeight;
  }, [entries]);

  async function submit() {
    const command = input.trim();
    if (command === "" || running) return;

    if (recall.current[recall.current.length - 1] !== command) recall.current = [...recall.current, command].slice(-HISTORY_LIMIT);
    pointer.current = -1;
    draft.current = "";
    setInput("");

    const id = nextId.current++;
    const entry: Entry = { id, db, command, outcome: { state: "running" } };
    setEntries((list) => [...list, entry].slice(-ENTRY_LIMIT));
    setRunning(true);

    let outcome: Outcome;
    try {
      const result = await api.post<{ reply: Reply }>("/redis/command", { db, command });
      outcome = { state: "reply", reply: result.reply };
      // Komut veri değiştirmiş olabilir; gezginin önbelleği eskimiş sayılır.
      void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
      void queryClient.invalidateQueries({ queryKey: ["redis", "key"] });
      void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
    } catch (error) {
      // Engellenen komutlar ve kapanmamış tırnaklar (HTTP 400) bildirim değil, geçmişte hata olarak görünür.
      outcome = { state: "failed", message: errorMessage(error) };
    }
    setEntries((list) => list.map((entry) => (entry.id === id ? { ...entry, outcome } : entry)));
    setRunning(false);
    inputRef.current?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      void submit();
    } else if (event.key === "ArrowUp") {
      const list = recall.current;
      if (list.length === 0) return;
      event.preventDefault();
      if (pointer.current === -1) draft.current = input;
      pointer.current = Math.min(pointer.current + 1, list.length - 1);
      setInput(list[list.length - 1 - pointer.current] ?? "");
    } else if (event.key === "ArrowDown") {
      if (pointer.current === -1) return;
      event.preventDefault();
      pointer.current -= 1;
      setInput(pointer.current === -1 ? draft.current : (recall.current[recall.current.length - 1 - pointer.current] ?? ""));
    }
  }

  function fillExample(command: string) {
    setInput(command);
    inputRef.current?.focus();
  }

  const prompt = `db${db}>`;

  return (
    <div className="flex h-[calc(100dvh-18rem)] min-h-[24rem] flex-col gap-3">
      <div className="flex items-center justify-end gap-2">
        <Button disabled={entries.length === 0} onClick={() => setEntries([])}>
          {t("Temizle")}
        </Button>
        <DbSelect value={db} info={info} onChange={setDb} />
      </div>

      <section className="flex min-h-0 flex-1 flex-col rounded-lg border border-line bg-surface">
        <div
          ref={screenRef}
          className="min-h-0 flex-1 overflow-y-auto p-3 font-mono text-[12.5px] leading-relaxed"
          onClick={() => {
            // Metin seçilirken odak çalınmasın.
            if (!window.getSelection()?.toString()) inputRef.current?.focus();
          }}
        >
          {entries.length === 0 ? (
            <div className="flex flex-col items-start gap-2 font-sans">
              <p className="text-ink-2">{t("Bir komut yazıp Enter'a basın. Örnekler:")}</p>
              <div className="flex flex-wrap gap-2">
                {EXAMPLES.map((example) => (
                  <button
                    key={example}
                    type="button"
                    onClick={(event) => {
                      event.stopPropagation();
                      fillExample(example);
                    }}
                    className="rounded-md border border-line-strong bg-surface px-2.5 py-1 font-mono text-[12.5px] transition-colors hover:bg-sunken"
                  >
                    {example}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            entries.map((entry) => <EntryView key={entry.id} entry={entry} />)
          )}
        </div>

        <div className="border-t border-line px-3 py-2">
          <p className="mb-1.5 text-[12.5px] text-ink-3">{t("Sunucuyu durduran ya da yapılandırmayı değiştiren komutlar konsoldan çalıştırılamaz; yapılan her komut denetim günlüğüne yazılır.")}</p>
          <label className="flex items-center gap-2">
            <span className="shrink-0 font-mono text-[12.5px] text-accent">{prompt}</span>
            <input
              ref={inputRef}
              value={input}
              aria-label={t("Redis komutu")}
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              readOnly={running}
              onChange={(event) => setInput(event.target.value)}
              onKeyDown={onKeyDown}
              className="h-8 min-w-0 flex-1 bg-transparent font-mono text-[12.5px] text-ink outline-none placeholder:text-ink-3"
            />
            {running && <Spinner className="text-ink-3" />}
          </label>
        </div>
      </section>
    </div>
  );
}
