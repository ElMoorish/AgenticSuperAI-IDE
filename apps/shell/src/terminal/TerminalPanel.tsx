import { Check, Loader, Plus, X } from "lucide-react";
import { useEffect, useState } from "react";
import { AgentTerminal } from "./AgentTerminal";
import { startAgentTerminalFeed, useAgentTerminals } from "./agentTerminals";
import { UserTerminal } from "./UserTerminal";

const MAX_USER = 8;
type Sel = { kind: "user"; n: number } | { kind: "agent"; id: number };

export function TerminalPanel() {
  const agent = useAgentTerminals((s) => s.terms);
  const remove = useAgentTerminals((s) => s.remove);
  const [users, setUsers] = useState<number[]>([1]);
  const [next, setNext] = useState(2);
  const [sel, setSel] = useState<Sel>({ kind: "user", n: 1 });

  useEffect(() => startAgentTerminalFeed(), []);

  const addUser = () => {
    if (users.length >= MAX_USER) return;
    setUsers([...users, next]);
    setSel({ kind: "user", n: next });
    setNext(next + 1);
  };
  const closeUser = (n: number) => {
    const rest = users.filter((u) => u !== n);
    setUsers(rest);
    if (sel.kind === "user" && sel.n === n) {
      setSel(rest.length ? { kind: "user", n: rest[rest.length - 1] } : agent.length ? { kind: "agent", id: agent[0].id } : { kind: "user", n: -1 });
    }
  };
  const isUser = (n: number) => sel.kind === "user" && sel.n === n;
  const isAgent = (id: number) => sel.kind === "agent" && sel.id === id;

  return (
    <>
      <div className="ptabs" role="tablist" aria-label="Terminals">
        {users.map((n) => (
          <span key={`u${n}`} className="ptab">
            <button type="button" role="tab" aria-selected={isUser(n)} onClick={() => setSel({ kind: "user", n })}>Terminal {n}</button>
            <button type="button" className="x" aria-label={`Close terminal ${n}`} onClick={() => closeUser(n)}><X size={12} aria-hidden="true" /></button>
          </span>
        ))}
        {agent.map((t) => (
          <span key={`a${t.id}`} className="ptab">
            <button type="button" role="tab" aria-selected={isAgent(t.id)} onClick={() => setSel({ kind: "agent", id: t.id })}>
              Agent #{t.id}{t.exited ? <Check size={12} aria-label=" finished" /> : <Loader size={12} className="spin" aria-label=" running" />}
            </button>
            <button
              type="button"
              className="x"
              aria-label={`Close agent terminal ${t.id}`}
              onClick={() => { remove(t.id); if (isAgent(t.id)) setSel({ kind: "user", n: users[0] ?? -1 }); }}
            ><X size={12} aria-hidden="true" /></button>
          </span>
        ))}
        <button type="button" className="padd" aria-label="New terminal" title="New terminal" disabled={users.length >= MAX_USER} onClick={addUser}><Plus size={14} aria-hidden="true" /></button>
      </div>
      <div className="pbody">
        {users.map((n) => (
          <div key={n} className="term-pane" hidden={!isUser(n)}>
            <UserTerminal active={isUser(n)} />
          </div>
        ))}
        {agent.map((t) => (
          <div key={t.id} className="term-pane" hidden={!isAgent(t.id)}>
            <AgentTerminal term={t} />
          </div>
        ))}
        {users.length === 0 && agent.length === 0 && <p className="term-empty">No terminals open. Press + to start one.</p>}
      </div>
    </>
  );
}
