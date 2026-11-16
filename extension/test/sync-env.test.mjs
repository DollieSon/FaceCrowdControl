import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseEnv, serverUrlFrom } from '../scripts/sync-env.mjs';

test('parseEnv reads plain, quoted, exported and commented lines', () => {
    const env = parseEnv([
        '# comment',
        'PORT=4000',
        'export BIND_ADDRESS="127.0.0.1"',
        "PGPASSWORD='p#ss'",
        'PGUSER=user # trailing comment',
        '',
        'not a line',
    ].join('\r\n'));
    assert.deepEqual(env, { PORT: '4000', BIND_ADDRESS: '127.0.0.1', PGPASSWORD: 'p#ss', PGUSER: 'user' });
});

test('serverUrlFrom builds the URL the extension posts to', () => {
    assert.equal(serverUrlFrom({}), 'http://127.0.0.1:3000');
    assert.equal(serverUrlFrom({ PORT: '4000' }), 'http://127.0.0.1:4000');
    assert.equal(serverUrlFrom({ BIND_ADDRESS: '0.0.0.0', PORT: '5000' }), 'http://127.0.0.1:5000');
    assert.equal(serverUrlFrom({ BIND_ADDRESS: 'localhost' }), 'http://localhost:3000');
    assert.equal(serverUrlFrom({ EXTENSION_SERVER_URL: 'http://192.168.1.5:3000/', PORT: '1' }), 'http://192.168.1.5:3000');
});
