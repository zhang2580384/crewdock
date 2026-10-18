import { defineStore } from 'pinia';
import { api } from '../api';
import type { Company, ProjectListItem, SessionUser } from '../types';

function storageKey(user: SessionUser) {
  return `crewdock-project:${user.organization_id}:${user.id}`;
}

export const useWorkspaceStore = defineStore('workspace', {
  state: () => ({
    company: null as Company | null,
    projects: [] as ProjectListItem[],
    projectId: '',
    loading: false,
    initializedFor: '',
  }),
  getters: {
    selectedProject(state) {
      return state.projects.find((project) => project.id === state.projectId) ?? null;
    },
  },
  actions: {
    async initialize(user: SessionUser, force = false) {
      if (!force && this.initializedFor === user.id) return;
      this.loading = true;
      try {
        const [company, projects] = await Promise.all([api.company(), api.projects()]);
        this.company = company;
        this.projects = projects;
        const saved = localStorage.getItem(storageKey(user)) || '';
        this.projectId = projects.some((project) => project.id === saved)
          ? saved
          : projects[0]?.id || '';
        this.initializedFor = user.id;
      } finally {
        this.loading = false;
      }
    },
    selectProject(user: SessionUser, projectId: string) {
      if (projectId && !this.projects.some((project) => project.id === projectId)) return;
      this.projectId = projectId;
      if (projectId) localStorage.setItem(storageKey(user), projectId);
      else localStorage.removeItem(storageKey(user));
    },
    reset() {
      this.company = null;
      this.projects = [];
      this.projectId = '';
      this.initializedFor = '';
    },
  },
});
