const fs = require('fs');
const path = require('path');
// uncomment if running locally
// const dir_path = ""
const filePath = path.join(__dirname, 'data', 'webhook_data.json');

function writeWebhookData(data) {
    fs.writeFile(filePath, JSON.stringify(data, null, 2), (err) => {
        if (err) {
            console.error('Error writing webhook data:', err);
        } else {
            console.log('Webhook data written successfully');
        }
    });
}

let loops = 0;
setInterval(() => {
    const data = {
        timestamp: new Date().toISOString(),
        message: `loops ${loops++}`,
    };
    writeWebhookData(data);
}, 5000); 

console.log(`Webhook data will be written to ${filePath}`);