const api_url = document.location.href.replace(/[^/]*$/, 'api');

export const searchParams = () => (
    new URL(location.href).searchParams
)

export async function login() {
    const cfg = await (await fetch("/api/config/public")).json()
    if (cfg && cfg.cas_prefix_url) {
        document.location.href = cfg.cas_prefix_url + "/login?service=" + api_url + "/login"
    }
}

export async function api(api_function, params) {
    const url = new URL(api_url + '/' + api_function);
    for (const key in params) {
        url.searchParams.set(key, params[key]);
    }
    const response = await fetch(url.toString());
    if (response.status === 200) {
        const json = await response.json()
        console.log(json)
        return json
    }
    if (response.status === 401) {
        await login();
        return new Promise(_ => {}) // return dead promise
    }
    throw new Error(response.toString())
}

export const to_valid_DOM_id = (id) => (
    id.replace(/[^a-z0-9_]/gi, '_')
)

export function create_dynamic_template(id, template) {
    const id_ = to_valid_DOM_id(id)

    const elt = document.createElement("template");
    elt.setAttribute("id", id_)
    elt.innerHTML = template
    document.body.appendChild(elt)
    return id_
}