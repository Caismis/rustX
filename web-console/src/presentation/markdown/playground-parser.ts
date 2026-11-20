import { ExpressParser } from '@a2ui/agent';
import { Catalog } from '@a2ui/web_core/catalog';
import catalog from './vendor/playground/catalog.json';

export const playgroundParser = new ExpressParser(Catalog.fromJson(catalog, '1.0', catalog.catalogId), 'inline', '1.0');
