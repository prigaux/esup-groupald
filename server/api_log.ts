import * as fs from 'fs'
import { promisify } from 'util'

import conf from './conf'
import { Dn, hLoggedUser, LoggedUser, Mright, Option, SgroupLogs } from './my_types';

function sgroup_log_file(log_dir: string, id: string, opts?: { sync: true }): string {
    id = id.replace('/', "_"); // it should not be necessary but...
    return `${log_dir}/${id}${opts?.sync ? '-sync': ''}.jsonl`
}

function blank_partial_line(b: Buffer) {
    const [lf, space] = ["\n", " "].map(s => s.charCodeAt(0))
    let i
    for (i = 0; b[i] !== lf; i++) b[i] = space
    b[i] = space
}

async function read_full_lines(file_path: string, bytes: number) {
    const f = await promisify(fs.open)(file_path, 'r')
    const stat = await promisify(fs.fstat)(f)
    const last_log_date = stat.mtime
    const whole_file = bytes > stat.size

    const buffer = Buffer.alloc(Math.min(bytes, stat.size))
    if (bytes) {
        await promisify(fs.read)(f, whole_file ? { buffer } : { buffer, length: bytes, position: stat.size - bytes })
        if (!whole_file) blank_partial_line(buffer)
    }

    return { last_log_date, whole_file, buffer }
}

function parse_jsonl(jsonl: string) {
    const json = '[' + jsonl.trimEnd().replace(/\n/g, ",") + ']'
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return JSON.parse(json) as any[]
}

async function read_jsonl(file_path: string, bytes: number): Promise<SgroupLogs> {
    try {
        const { buffer, ...o } = await read_full_lines(file_path, bytes)
        // eslint-disable-next-line @typescript-eslint/no-unsafe-return
        return { logs: parse_jsonl(buffer.toString()), ...o }
    } catch (err) {
        // @ts-expect-error
        if (err?.code === 'ENOENT') {
            return { last_log_date: undefined, whole_file: true, logs: [] }
        }
        throw err
    }
}

async function audit(file: string, msg: string) {
    await promisify(fs.writeFile)(file, msg + "\n", { flag: "a" })
}

/**
 * Read log entries
 * @param id - sgroup identifier
 * @param bytes - maximum number of bytes to read
 * @returns log entries
 */
export async function get_sgroup_logs(id: string, bytes: number, opts?: { sync: true }) {
    if (!conf.log_dir) throw `you must configure conf.log_dir first`
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return await read_jsonl(sgroup_log_file(conf.log_dir, id, opts), bytes)
}

export type action = 'create' | 'modify_attrs' | 'delete' | 'modify_members_or_rights' | 'modify_remote_query'

/**
 * Add a log entry
 * @param user - who did the action
 * @param id - group/stem identifier
 * @param action - one of "create", "delete"...
 * @param msg - optional message explaining why the user did this action
 * @param data - info about this action (content depends on the action)
 */
export async function log_sgroup_action(user: LoggedUser, id: string, action: action, msg: Option<string>, data: Record<string, any>, when?: Date) {
    if (conf.log_dir) {
        const who = hLoggedUser.toString(user)
        when ??= new Date()
        await audit(
            sgroup_log_file(conf.log_dir, id),
            JSON.stringify({ action, when, who, msg, ...data }))
    }
}

/**
 * Add a log entry
 * @param id - group identifier
 */
export async function log_sgroup_flattened_modifications(id: string, mright: Mright, data: { new_count: number, added: Dn[], removed: Dn[] }) {
    if (conf.log_dir) {
        const when = new Date()
        await audit(
            sgroup_log_file(conf.log_dir, id, { sync: true }),
            JSON.stringify({ when, mright, ...data }))
    }
}

export const export_for_tests = { parse_jsonl, blank_partial_line }
