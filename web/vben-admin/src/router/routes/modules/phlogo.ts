import type { RouteRecordRaw } from 'vue-router';

/**
 * phlogo admin routes (p6-02): Dashboard / Sources / Nodes / NodeDetail / Checks / Scoring / Subscriptions.
 * Endpoint mappings per docs/rebuild/07-p6-admin-ui/p6-02-pages.md.
 */
const routes: RouteRecordRaw[] = [
  {
    meta: {
      icon: 'lucide:list',
      order: 900,
      title: 'Phlogo',
    },
    name: 'PhlogoRoot',
    path: '/phlogo',
    children: [
      {
        meta: { title: 'Dashboard' },
        name: 'PhlogoDashboard',
        path: 'dashboard',
        component: () => import('#/views/phlogo/dashboard.vue'),
      },
      {
        meta: { title: 'Sources' },
        name: 'PhlogoSources',
        path: 'sources',
        component: () => import('#/views/phlogo/sources.vue'),
      },
      {
        meta: { title: 'Nodes' },
        name: 'PhlogoNodes',
        path: 'nodes',
        component: () => import('#/views/phlogo/nodes.vue'),
      },
      {
        meta: { title: 'Node Detail', hideInMenu: true },
        name: 'PhlogoNodeDetail',
        path: 'nodes/:id',
        component: () => import('#/views/phlogo/node-detail.vue'),
      },
      {
        meta: { title: 'Checks' },
        name: 'PhlogoChecks',
        path: 'checks',
        component: () => import('#/views/phlogo/checks.vue'),
      },
      {
        meta: { title: 'Scoring' },
        name: 'PhlogoScoring',
        path: 'scoring',
        component: () => import('#/views/phlogo/scoring.vue'),
      },
      {
        meta: { title: 'Subscriptions' },
        name: 'PhlogoSubscriptions',
        path: 'subscriptions',
        component: () => import('#/views/phlogo/subscriptions.vue'),
      },
      {
        meta: { title: 'Sponsors' },
        name: 'PhlogoSponsors',
        path: 'sponsors',
        component: () => import('#/views/phlogo/sponsors.vue'),
      },
    ],
  },
];

export default routes;
