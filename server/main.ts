import express from 'express';
import path from 'path';
import bodyParser from 'body-parser'
import api_routes from './api_routes'
import conf from './conf';
import * as express_helpers from './express_helpers'


const staticFilesOptions = { maxAge: process.env.NODE_ENV === 'production' ? 60 * 60 * 1000 : 0 };

const index_html = (_req: express.Request, res: express.Response): void => {
    res.sendFile(path.join(__dirname, "../ui/dist/index.html"), err => {
        if (err) console.error(err)
    })
};

//thread::spawn(move || cron::the_loop(config, all_caches));

const app = express();

if (conf.trust_proxy) app.set('trust proxy', conf.trust_proxy)
app.use(express_helpers.session_store());

app.use("/api", 
    bodyParser.json({type: '*/*'}), // do not bother checking, everything we will get is JSON :)
    api_routes)
app.use("/", express.static(path.join(__dirname, '../ui/dist'), staticFilesOptions));
app.get(/[/](sgroup|new_sgroup|sgroup_history)$/, // keep in sync with ui/src/router/index.ts "routes" "path"
        index_html)

const port = process.env.PORT || 8080;        // set our port
app.listen(port);
console.log('Started on port', port);
