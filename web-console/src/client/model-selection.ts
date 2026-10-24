import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { isOutcomeUncertain } from './errors';
import type { AttachmentTarget, SessionModelConfig } from '../../../protocol/app-server/v57';
import type { SessionLifecycles } from './session-lifecycle/system';
import type { ClientView, ConnectionRequest, SessionView } from './contracts';

/** What Session model selection reads from and writes to its client. */
export interface ModelSelectionPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  setSession(id: string, patch: Partial<Pick<SessionView, 'modelIntent' | 'modelMutation'>>): void;
  target(id: string): AttachmentTarget;
  observe: SessionLifecycles['observe'];
  epoch(id: string): number | undefined;
  waitForAttachment(id: string, current: () => boolean): Promise<void>;
  refresh(id: string): Promise<void>;
}

/** Session model selection: the client-owned choice made while a Session's
 * resources initialize, native model mutations and their authoritative
 * reconciliation. A mutation response alone never confirms a model. */
export class ModelSelection {
  constructor(private readonly port: ModelSelectionPort) {}
  private modelPreparations = new Map<string, { work: Promise<SessionModelConfig>; retire: () => void }>();
  /** Client-owned selection while native resources initialize. Last unsent choice
   * wins; the existing native mutation and authoritative reread still own apply. */
  prepareAgentModel(id: string, config: SessionModelConfig) {
    const view = this.port.state().views[id];
    if (view?.attachment !== 'attaching' || view.attachmentIntent !== 'wanted' || view.deleting)
      return Promise.reject(new Error('Conversation is no longer connecting.'));
    this.port.setSession(id, { modelIntent: { config, phase: 'waiting' } });
    const previous = this.modelPreparations.get(id);
    if (previous) return previous.work;
    let live = true;
    const generation = this.port.state().generation, epoch = this.port.epoch(id);
    const current = () => live && this.port.current(generation) && this.port.epoch(id) === epoch
      && this.port.state().views[id]?.attachmentIntent === 'wanted' && !this.port.state().views[id]?.deleting;
    const work = (async () => {
      try {
        await this.port.waitForAttachment(id, current);
        if (!current()) throw new Error('Model selection retired with its connection.');
        const selection = this.port.state().views[id].modelIntent!.config;
        this.port.setSession(id, { modelIntent: { config: selection, phase: 'applying' } });
        await this.setAgentModel(id, selection);
        if (!current()) throw new Error('Model selection retired with its connection.');
        await this.repairAgentModel(id);
        if (!current()) throw new Error('Model selection retired with its connection.');
        const observed = this.port.state().views[id].snapshot?.model?.configured;
        if (observed?.model !== selection.model || (observed.profile ?? undefined) !== (selection.profile ?? undefined))
          throw new Error('Selected model has not been confirmed. Reread model configuration before sending.');
        this.port.setSession(id, { modelIntent: undefined });
        return selection;
      } catch (error) {
        const intent = this.port.state().views[id]?.modelIntent;
        if (current() && intent) this.port.setSession(id, { modelIntent: { ...intent, phase: 'failed', error: String(error) } });
        throw error;
      }
    })();
    this.modelPreparations.set(id, { work, retire: () => { live = false; } });
    void work.finally(() => { if (this.modelPreparations.get(id)?.work === work) this.modelPreparations.delete(id); }).catch(() => {});
    return work;
  }
  /** Transport continuation guard, not Session model authority. A successful
   * mutation response alone cannot enable a dependent Send. */
  async setAgentModel(id: string, config: SessionModelConfig) {
    const target = this.port.target(id), generation = this.port.state().generation;
    if (this.port.state().views[id].modelMutation) throw new Error('Reread native model state before another mutation.');
    const current = () => this.port.current(generation) && sameTarget(this.port.state().views[id]?.target, target);
    const operation = { generation, status: 'in-flight' as const };
    this.port.setSession(id, { modelMutation: operation });
    try {
      await this.port.request({ method: 'session/setModel', params: { target, config } }, 'model_set');
      if (!current()) return;
      this.port.setSession(id, { modelMutation: { generation, status: 'acknowledged' }, ...(this.port.state().views[id]?.modelIntent?.phase === 'failed' ? { modelIntent: undefined } : {}) });
    } catch (error) {
      if (this.port.state().views[id]?.modelMutation === operation) this.port.setSession(id, { modelMutation: { generation, status: isOutcomeUncertain(error) ? 'uncertain' : 'acknowledged' } });
      throw error;
    }
  }
  async repairAgentModel(id: string) {
    const view = this.port.state().views[id], target = view?.target;
    if (!target || !this.port.observe(id)) return;
    const generation = this.port.state().generation, operation = view.modelMutation, intent = view.modelIntent;
    if (operation?.status === 'in-flight') return;
    await this.port.refresh(id);
    const latest = this.port.state().views[id];
    // Recovery installs a fresh observation proof. Fence this reconciliation
    // by its connection, attachment and exact operation, rather than the proof
    // that the authoritative checkpoint necessarily replaced.
    if (!this.port.current(generation) || !sameTarget(latest?.target, target)
      || latest.modelMutation !== operation || latest.modelIntent !== intent
      || latest.observationState !== 'following') return;
    const observed = latest.snapshot?.model?.configured;
    const confirmed = intent?.phase === 'failed' && observed?.model === intent.config.model
      && (observed.profile ?? undefined) === (intent.config.profile ?? undefined);
    if (operation || confirmed) this.port.setSession(id, { modelMutation: undefined, ...(confirmed ? { modelIntent: undefined } : {}) });
  }
  /** A model mutation of an earlier connection generation is never current. */
  reconcile(id: string) {
    const mutation = this.port.state().views[id].modelMutation;
    if (mutation && mutation.generation !== this.port.state().generation) this.port.setSession(id, { modelMutation: undefined });
  }
  /** The selection a pending preparation will apply before a send. */
  preparation(id: string) { return this.modelPreparations.get(id)?.work; }
  /** A released Session keeps no preparation. */
  release(id: string) { this.modelPreparations.get(id)?.retire(); this.modelPreparations.delete(id); }
  /** A lost connection retires every preparation. */
  disconnect() { for (const pending of this.modelPreparations.values()) pending.retire(); this.modelPreparations.clear(); }
}
