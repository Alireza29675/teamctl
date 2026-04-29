import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import react from '@astrojs/react';

export default defineConfig({
  site: 'https://teamctl.run',
  integrations: [
    react(),
    starlight({
      title: 'teamctl',
      description: 'docker-compose for persistent AI agent teams.',
      social: {
        github: 'https://github.com/Alireza29675/teamctl',
      },
      editLink: {
        baseUrl: 'https://github.com/Alireza29675/teamctl/edit/main/docs/',
      },
      sidebar: [
        { label: 'Getting started', link: '/getting-started/' },
        {
          label: 'Operating',
          items: [
            { label: 'Cost & rate limits', link: '/cost/' },
            { label: 'Coordination policy', link: '/coordination-policy/' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { label: 'Projects', link: '/concepts/projects/' },
            { label: 'Channels', link: '/concepts/channels/' },
            { label: 'Runtimes', link: '/concepts/runtimes/' },
            { label: 'Bridges', link: '/concepts/bridges/' },
            { label: 'The .team/ folder', link: '/concepts/team-folder/' },
            { label: 'Interfaces', link: '/concepts/interfaces/' },
            { label: 'HITL', link: '/concepts/hitl/' },
            { label: 'Rate limits', link: '/concepts/rate-limits/' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'team-compose.yaml', link: '/reference/team-compose-yaml/' },
            { label: 'teamctl CLI', link: '/reference/teamctl/' },
            { label: 'runtimes/*.yaml', link: '/reference/runtimes-yaml/' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Your first team', link: '/guides/first-team/' },
            { label: 'Multi-runtime teams', link: '/guides/multi-runtime/' },
            { label: 'Telegram bot setup', link: '/guides/telegram-bot/' },
            { label: 'Bridges and HITL', link: '/guides/bridges-and-hitl/' },
            { label: 'Operating in production', link: '/guides/operating-in-production/' },
          ],
        },
        {
          label: 'Cookbook',
          items: [
            { label: 'Multi-agent ACLs in one project', link: '/cookbook/multi-agent/' },
            { label: 'Mixing runtimes in one team', link: '/cookbook/multi-runtime/' },
            { label: 'Two projects, one teamctl, with bridges', link: '/cookbook/two-projects/' },
          ],
        },
        {
          label: 'ADRs',
          autogenerate: { directory: 'adrs' },
        },
      ],
      customCss: [],
    }),
  ],
});
