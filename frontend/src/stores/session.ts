import { defineStore } from 'pinia';
import { api, clearToken, setToken } from '../api';
import type { SessionUser } from '../types';
import { useWorkspaceStore } from './workspace';

let loadMePromise: Promise<void> | null = null;

export const useSessionStore = defineStore('session', {
  state: () => ({
    user: null as SessionUser | null,
    loading: false,
  }),
  actions: {
    async login(email: string, password: string) {
      this.loading = true;
      try {
        const data = await api.login(email, password);
        setToken(data.token);
        await this.loadMe(true);
      } catch (error) {
        clearToken();
        throw error;
      } finally {
        this.loading = false;
      }
    },
    async loadMe(force = false) {
      if (this.user && !force) return;
      if (!loadMePromise) {
        loadMePromise = (async () => {
          const data = await api.me();
          this.user = data.user;
          await useWorkspaceStore().initialize(data.user, force);
        })().finally(() => {
          loadMePromise = null;
        });
      }
      return loadMePromise;
    },
    async logout() {
      try {
        await api.logout();
      } finally {
        this.clearLocalSession();
      }
    },
    clearLocalSession() {
      clearToken();
      this.user = null;
      useWorkspaceStore().reset();
    },
  },
});
