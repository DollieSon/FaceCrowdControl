const express = require('express');
const app = express();

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
    res.send('Hello, Worldies!');
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