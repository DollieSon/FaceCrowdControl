const express = require('express');
const app = express();
const {givePointsToUser} = require('./commands'); 
const { Client } = require('pg');

const client = new Client({
    user: 'user',
    host: 'db',
    database: 'facecrowdcontrol',
    password: 'password',
    port: 5432,
})

function incrementPoint(dbclient){
    const text = 'UPDATE users SET point = point + 1 WHERE LatestInteraction >= NOW() - INTERVAL \'30 minutes\' RETURNING *;';
    dbclient.query(text)
        .then(res => {
            console.log('Point incremented:', res.rows[0]);
        })
        .catch(err => {
            console.error('Error executing query', err.stack);
        });
}


client.connect()
    .then(() => {
        console.log('Connected to PostgreSQL')
        client.query(`
            CREATE TABLE IF NOT EXISTS users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                point INTEGER,
                createdAt TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                latestInteraction TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            `);
        setInterval(()=>{incrementPoint(client)}, 3 * 60 * 1000); // Increment points every 3 minutes
    })
    .catch(err => console.error('Connection error', err.stack));

app.get('/', (req, res) => {
    // give the currrent time
    time = new Date();
    res.send(`Hello, Worldies! Current time: ${time}`);
});

app.post('/users', express.json(), async (req, res) => {
    const { name, age } = req.body;
    console.log(`Received user: ${name}, age: ${age}`);
    try {
        const text = `
            INSERT INTO users (name, point)
            VALUES ($1, $2)
        `;
        const values = [name, age];
        const result = await client.query(text, values);
        res.status(201).json(result.rows[0]);
    } catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error');
    }
});

commands = {
    'give': {
        parameters: ['targetId', 'amount'],
        function : givePointsToUser
    }
}

app.post('/fb_message',express.json(), async (req, res) => {
    const { fbname, message } = req.body;
    console.log(`Received message from ${fbname}: ${message}`);
    // check if the user exists
    try {
        const text = 'SELECT * FROM users WHERE name = $1';
        const values = [fbname];
        const result = await client.query(text, values);
        if (result.rows.length > 0) {
            // User exists, update latest interaction
            const updateText = 'UPDATE users SET latestInteraction = NOW() WHERE name = $1 RETURNING *';
            const updateValues = [fbname];
            const updateResult = await client.query(updateText, updateValues);
            // res.status(200).json(updateResult.rows[0]);

            // check if the first ch is / and next word is a command
            if (message.startsWith('/')) {
                const parts = message.split(' ');
                const command = parts[0].substring(1); // Remove the leading '/'
                const args = parts.slice(1);

                if (commands[command]) {
                    console.log('Command found:', command);
                    console.log('Command object:', JSON.stringify(commands[command]));
                    
                    // Safely destructure with checks
                    const commandObj = commands[command];
                    const parameters = commandObj.parameters;
                    const comfunction = commandObj['function'];
                    
                    console.log('Parameters:', parameters);
                    console.log('Function:', typeof comfunction);
                    
                    if (!parameters) {
                        return res.status(500).send(`Command ${command} has no parameters defined`);
                    }
                    
                    console.log(`Executing command: ${command} with args: ${args}`);
                    if (args.length !== parameters.length) {
                        return res.status(400).send(`Invalid number of arguments for command ${command}`);
                    }
                    try {
                        if (!comfunction) {
                            return res.status(500).send(`Command ${command} has no function defined`);
                        }
                        
                        console.log('About to execute function');
                        const result = await comfunction(client, fbname, ...args);
                        return res.status(200).json(result);
                    } catch (error) {
                        console.error('Command execution error:', error);
                        return res.status(500).send(`Error executing command: ${error.message}`);
                    }
                } else {
                    return res.status(400).send(`Unknown command: ${command}`);
                }
            }else{
                // If no command, just return the user info
                return res.status(200).json(updateResult.rows[0]);
            }
        } else {
            // User does not exist, create new user
            const insertText = 'INSERT INTO users (name, point) VALUES ($1, 0) RETURNING *';
            const insertValues = [fbname];
            const insertResult = await client.query(insertText, insertValues);
            res.status(201).json(insertResult.rows[0]);
        }
    }catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error: query failed');
    }
});

app.get('/users', async (req, res) => {
    try {
        const result = await client.query('SELECT * FROM users');
        res.status(200).json(result.rows);
    } catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error');
    }
});

const port = process.env.PORT || 3000;

app.listen(port, () => {
    console.log(`Server is runnin on http://localhost:${port}`);
});