<script lang="ts">
import { cloneDeep, fromPairs, isEmpty, isEqual, last, mapValues, omit, pickBy } from 'lodash'
import { computed, defineAsyncComponent, defineComponent, ref } from 'vue'
import router from '@/router';
import { asyncComputed_, ref_watching } from '@/vue_helpers';
import { forEachAsync, addDays } from '@/helpers';
import { DirectOptions, Dn, LdapConfigOut, Mright, MyMod, PRecord, SgroupAndMoreOut_ } from '@/my_types';
import { list_of_rights, right2text } from '@/lib';
import { mrights_flat_or_not } from '@/composition/SgroupSubjects';

import * as api from '@/api'

async function get_sgroup(id: string): Promise<SgroupAndMoreOut_> {
    let sgroup = { id, ...await api.sgroup(id) }
    if (sgroup.group) {
        sgroup.group.direct_members = await api.add_sscfg_dns_and_sort(sgroup.group.direct_members)
    }
    if (sgroup.synchronizedGroup) {
        api.convert.remote_sql_query.from_api(sgroup.synchronizedGroup.remote_sql_query)
        sgroup.synchronized_group_orig = cloneDeep(sgroup.synchronizedGroup)
    }
    return sgroup
}

export async function computedProps(to: RouteLocationNormalized) {
    const id = to.query.id as string
    return { 
        id, 
        initial_sgroup: await get_sgroup(id),
        ldapCfg: await api.config_ldap(),
    }
}

</script>

<script setup lang="ts">
import { vFocus, vClickWithoutMoving } from '@/vue_helpers';
import SgroupLink from '@/components/SgroupLink.vue';
import MyIcon from '@/components/MyIcon.vue';
import SearchSubjectToAdd from '@/components/SearchSubjectToAdd.vue';
import SgroupSubjects from './SgroupSubjects.vue';
import SgroupRightsView from './SgroupRightsView.vue';
import { RouteLocationNormalized } from 'vue-router';
const SynchronizedGroupView = defineAsyncComponent(() => import('./SynchronizedGroupView.vue'));

const props = withDefaults(defineProps<{
  tabToDisplay: 'direct'|'rights',
  // computedProps
  id: string,
  initial_sgroup: SgroupAndMoreOut_,
  ldapCfg: LdapConfigOut,
}>(), { tabToDisplay: 'direct' });

// check coherence of props of prefetch
((_: Awaited<ReturnType<typeof computedProps>>) => {})(props)

const set_tabToDisplay = (tabToDisplay: 'direct'|'rights') => {
    const hash = tabToDisplay !== 'direct' ? '#tabToDisplay=' + tabToDisplay : ''
    router.push({ path: '/sgroup', query: { id: props.id }, hash })
}

let tabs = computed(() => {
    return {
        direct: sgroup.value.stem ? "Contenu du dossier" : "Membres",
        rights: 'Privilèges',
    }
})

let sgroup = ref_watching({ 
    value: () => props.initial_sgroup,
    update: () => get_sgroup(props.id),
})

let members = mrights_flat_or_not(props.ldapCfg, sgroup, 'member', 
                        () => !!sgroup.value.synchronizedGroup, 
                        () => sgroup.value.group?.direct_members || {})

let can_modify_member = computed(() => (
    sgroup.value && ['updater', 'admin'].includes(sgroup.value.right)) && !sgroup.value.synchronizedGroup
)

let add_member_show = ref(false)

async function add_remove_direct_mright(dn: Dn, mright: Mright, mod: MyMod, options: DirectOptions = {}) {
    console.log('add_remove_direct_mright')
    await api.modify_members_or_rights(props.id, { [mright]: { [mod]: { [dn]: options } } })
    if (mright === 'member') {
        sgroup.update()
    } else {
        rights_force_refresh.value++
    }
}
function add_direct_mright(dn: Dn, mright: Mright) {
    const end_days = mright === 'member' && (sgroup.value.attrs["groupaldOptions;x-member-ttl-default"] || sgroup.value.attrs["groupaldOptions;x-member-ttl-max"])
    const enddate = end_days && addDays(new Date(), parseInt(end_days)).toISOString()
    add_remove_direct_mright(dn, mright, 'add', enddate ? { enddate } : {})
}
function remove_direct_mright(dn: Dn, mright: Mright, options: DirectOptions = {}) {
    add_remove_direct_mright(dn, mright, 'delete', options)
}

let add_right_show = ref(false)
let rights_force_refresh = ref(0)
let direct_rights = asyncComputed_(async () => {
    rights_force_refresh.value // asyncComputed will know it needs to re-compute
    if (props.tabToDisplay !== 'rights') return;
    let r = await api.sgroup_direct_rights(props.id)
    await forEachAsync(r, (subjects, _) => api.add_sscfg_dns(subjects))
    return r
})
let rights = fromPairs(list_of_rights.map(right => (
    [ right, mrights_flat_or_not(props.ldapCfg, sgroup, right, () => false, () => direct_rights.value?.[right] || {}) ]
)))

const sgroup_attrs_templates = computed(() => (
    mapValues(props.ldapCfg.sgroup_attrs, (opts, _) => (
        defineComponent({
            props: ['value'],
            template: opts.vue_template || `{{value}}`
        })
    ))
))
const other_sgroup_attrs = computed(() => (
    pickBy(
        omit(props.ldapCfg.sgroup_attrs, 'cn', 'ou', 'description'), 
        (opts, _) => !opts.only_in_stem || props.id.startsWith(opts.only_in_stem)
    )
))
const other_sgroup_attrs_having_values = computed(() => (
    pickBy(other_sgroup_attrs.value, (_, attr) => sgroup.value.attrs[attr])
))

type AttrKind = 'ou'|'description'|'other'
const attrKind_to_attrs = (attrK: AttrKind) => (
    attrK === 'other' ? Object.keys(other_sgroup_attrs.value) : [attrK]
)
const get_attrK_value = (attrK: AttrKind) => (
    JSON.stringify(attrKind_to_attrs(attrK).map(attr => sgroup.value.attrs[attr]))
)
const set_attrK_value = (attrK: AttrKind, value: string) => {
    const values = JSON.parse(value)
    let i = 0
    for (const attr of attrKind_to_attrs(attrK)) {
        const value = values[i++]
        if (value === null) {
            delete sgroup.value.attrs[attr]
        } else {
            sgroup.value.attrs[attr] = value
        }
    }

}
let modify_attrs = ref_watching({ 
    watch: () => props.id, 
    value: () => ({} as PRecord<AttrKind, { prev: string, status?: 'canceling'|'saving' }>),
})
const start_modify_attr = (attrK: AttrKind) => {
    modify_attrs.value[attrK] = { prev: get_attrK_value(attrK) }
}
const cancel_modify_attr = (attrK: AttrKind, opt?: 'force') => {
    let state = modify_attrs.value[attrK]
    if (state) {
        if (get_attrK_value(attrK) !== state.prev && opt !== 'force') {
            if (state.status) return
            state.status = 'canceling'
            if (!confirm('Voulez vous perdre les modifications ?')) {
                setTimeout(() => { if (state?.status === 'canceling') state.status = undefined }, 1000)
                return
            }
        }
        // restore previous value
        set_attrK_value(attrK, state.prev)
    }
    // stop modifying this attrK:
    modify_attrs.value[attrK] = undefined
}
const delayed_cancel_modify_attr = (attrK: AttrKind) => {
    setTimeout(() => cancel_modify_attr(attrK), 200)
}
const send_modify_attr = async (attrK: AttrKind) => {
    let state = modify_attrs.value[attrK]
    if (state) state.status = 'saving'
    await api.modify_sgroup_attrs(props.id, sgroup.value.attrs)
    modify_attrs.value[attrK] = undefined
}

const delete_sgroup = async () => {
    if (confirm("Supprimer ?")) {
        await api.delete_sgroup(props.id)
        const parent = last(sgroup.value.parents)
        router.push(parent?.right ? { path: '/sgroup', query: { id: parent.sgroup_id } } : { path: "/" })
    }
}

const send_modify_synchronized_group = async () => {
    if (sgroup.value.synchronizedGroup) {
        await api.modify_remote_sql_query(props.id, sgroup.value.synchronizedGroup.remote_sql_query);
        sgroup.update()
    }
}
const cancel_modify_synchronized_group = () => {
    sgroup.update()
}

const transform_group_into_SynchronizedGroup = () => {
    delete sgroup.value.group;
    sgroup.value.synchronizedGroup = { remote_sql_query: {
        remote_cfg_name: '',
        select_query: '',
        to_subject_source: { ssdn: '', id_attr: '' },
    } }
}
const transform_SynchronizedGroup_into_group = async () => {
    if (confirm("Le groupe sera vide. Ok ?")) {
        await api.modify_members_or_rights(props.id, { member: { replace: {} } })
        sgroup.update()
    }
}

</script>

<template>
<div>
    <small class="float-right">
        ID : {{props.id}}
    </small>

    <a href=".">Accueil</a> &gt;
    <span v-for="(parent, i) in sgroup.parents">
        <SgroupLink :sgroup="parent" />
        <span v-if="i < sgroup.parents.length"> &gt; </span>
    </span>

    <h2>
        <MyIcon :name="sgroup.stem ? 'folder' : 'users'" class="on-the-left"
             :title="ldapCfg.sgroup_attrs.ou.description" />
    </h2>
    <template v-if="sgroup.right === 'admin'">
        <form v-if="modify_attrs.ou" @submit.prevent="send_modify_attr('ou')" class="modify_attr">
            <h2 :title="ldapCfg.sgroup_attrs.ou.description">
                <input v-focus v-model="sgroup.attrs.ou" @focusout="delayed_cancel_modify_attr('ou')" @keydown.esc="cancel_modify_attr('ou')">
            </h2>
            <MyIcon name="check" class="on-the-right" @click="send_modify_attr('ou')" />
            <MyIcon name="close" class="on-the-right" @click="cancel_modify_attr('ou', 'force')" />
        </form>
        <span v-else>
            <h2 v-click-without-moving="() => start_modify_attr('ou')">{{sgroup.attrs.ou}}</h2>
            <MyIcon name="pencil" class="on-the-right" @click="start_modify_attr('ou')" />
        </span>       
    </template>
    <h2 v-else>{{sgroup.attrs.ou}}</h2>

    <fieldset>
        <legend>
            <h4 :title="ldapCfg.sgroup_attrs.description.description">{{ldapCfg.sgroup_attrs.description.label}}</h4>
            <template v-if="sgroup.right === 'admin'">
                <template v-if="modify_attrs.description">
                    <MyIcon name="check" class="on-the-right" @click="send_modify_attr('description')" />
                    <MyIcon name="close" class="on-the-right" @click="cancel_modify_attr('description', 'force')" />
                </template>
                <template v-else>
                    <MyIcon name="pencil" class="on-the-right" @click="start_modify_attr('description')" />
                </template>
            </template>
        </legend>
        <textarea v-if="modify_attrs.description" class="description" v-model="sgroup.attrs.description" 
            v-focus @focusout="delayed_cancel_modify_attr('description')" @keydown.esc="cancel_modify_attr('description')"
            @keypress.enter.ctrl="send_modify_attr('description')"
            >
        </textarea>
        <div v-else class="description" v-click-without-moving="sgroup.right === 'admin' && (() => start_modify_attr('description'))">
            {{sgroup.attrs.description}}
        </div>
    </fieldset>

    <fieldset v-if="modify_attrs.other || !isEmpty(other_sgroup_attrs_having_values)">
        <legend>
            <h4>Divers</h4>
            <template v-if="modify_attrs.other">
                <MyIcon name="check" class="on-the-right" @click="send_modify_attr('other')" />
                <MyIcon name="close" class="on-the-right" @click="cancel_modify_attr('other', 'force')" />
            </template>
            <template v-else>
                <MyIcon name="pencil" class="on-the-right" @click="start_modify_attr('other')" />
            </template>
        </legend>
        <form v-if="modify_attrs.other" @submit.prevent="send_modify_attr('other')">
            <label class="label-and-val" v-for="(opts, attr) of other_sgroup_attrs">
                <span class="label" :title="opts.description">{{opts.label}}</span>
                <span>
                    <input v-model="sgroup.attrs[attr]" v-bind="opts.input_attrs">
                    <br>
                    <span>{{opts.description}}</span>
                </span>
            </label>
            <input type="submit" style="display: none;">
        </form>
        <template v-else>
            <template v-for="(opts, attr) of other_sgroup_attrs_having_values">
                <label class="label-and-val">
                    <span class="label" :title="opts.description">{{opts.label}}</span>
                    <component :value="sgroup.attrs[attr]" :is="sgroup_attrs_templates[attr]" />
                </label>
            </template>
        </template>
    </fieldset>

    <fieldset v-if="sgroup.synchronizedGroup">
        <legend>
            <h4>Synchronisation</h4>
            <template v-if="!isEqual(sgroup.synchronizedGroup, sgroup.synchronized_group_orig)">
                <MyIcon name="check" class="on-the-right" @click="send_modify_synchronized_group" />
                <MyIcon name="close" class="on-the-right" @click="cancel_modify_synchronized_group" />
            </template>
        </legend>

        <SynchronizedGroupView :id="props.id" :remote_sql_query="sgroup.synchronizedGroup.remote_sql_query" @save="send_modify_synchronized_group" />
    </fieldset>

    <p></p>
    <fieldset>
        <legend>
            <label v-for="(text, key) of tabs" @click="set_tabToDisplay(key)">
                <input type="radio" name="legend_choices" :value='key' :checked="tabToDisplay === key">
                    {{text}}
            </label>
        </legend>

        <div v-if="tabToDisplay === 'rights'">

            <button class="float-right" @click="add_right_show = !add_right_show" v-if="can_modify_member">{{add_right_show ? "Fermer l'ajout de droits" : "Ajouter des droits"}}</button>
            <p v-if="add_right_show" style="padding: 1rem; background: #eee">
                Recherchez un utilisateur/groupe/...<br>
                <p><SearchSubjectToAdd v-slot="{ dn, close }">
                    <template v-for="right of list_of_rights">               
                        <button @click.prevent="add_direct_mright(dn, right); close()">{{right2text[right]}}</button>
                        &nbsp;
                    </template>
                </SearchSubjectToAdd></p>
            </p>

            <span v-if="sgroup.stem">
                Les entités ayant des privilèges sur ce dossier <b>et tous les sous-dossiers et sous-groupes</b>
            </span>
            <span v-else>
                Les entités ayant des privilèges sur ce groupe
            </span>
            <SgroupRightsView v-if="direct_rights"
                :rights="rights"
                :can_modify="sgroup.right == 'admin'" @remove="remove_direct_mright" />
        </div>
        <ul v-else-if="sgroup.stem">
            <div v-if="isEmpty(sgroup.stem.children)"> <i>Vide</i> </div>
            <li v-for="(attrs, id) in sgroup.stem.children">
                <MyIcon name="folder" class="on-the-left" />
                <SgroupLink :sgroup="{ attrs }" :id="id" />
            </li>
        </ul>
        <div v-else-if="sgroup.group || sgroup.synchronizedGroup">
            <button class="float-right" @click="add_member_show = !add_member_show" v-if="can_modify_member">{{add_member_show ? "Fermer l'ajout de membres" : "Ajouter des membres"}}</button>
            <p v-if="add_member_show" style="padding: 1rem; background: #eee">
                Recherchez un utilisateur/groupe/...<br>
                <p><SearchSubjectToAdd v-slot="{ dn, close }">
                    <button @click.prevent="add_direct_mright(dn, 'member'); close()">Ajouter</button>
                </SearchSubjectToAdd></p>
            </p>
            <button class="float-right" @click="members.flat.show = !members.flat.show" v-if="members.details?.may_have_indirects && !sgroup.synchronizedGroup">{{members.flat.show ? "Cacher les indirects" : "Voir les indirects"}}</button>

            <SgroupSubjects :flat="members.flat" :results="members.results" :details="members.details" :can_modify="can_modify_member"
                @remove="(dn, opts) => remove_direct_mright(dn, 'member', opts)" />
        </div>
    </fieldset>

    <ul class="inline" v-if="sgroup.right === 'admin'">
        <template v-if="sgroup.stem">
            <li><RouterLink  :to="{ path: 'new_sgroup', query: { parent_id: props.id } }">
                <button>Créer un groupe</button>
            </RouterLink></li>

            <li><RouterLink  :to="{ path: 'new_sgroup', query: { parent_id: props.id, is_stem: true } }">
                <button>Créer un dossier</button>
            </RouterLink></li>
        </template>

        <template v-if="!sgroup.stem || isEmpty(sgroup.stem.children)">
            <li><button @click="delete_sgroup">Supprimer le {{sgroup.stem ? 'dossier' : 'groupe'}}</button></li>
        </template>

        <li><RouterLink target="_blank" :to="{ path: 'sgroup_history', query: { id: props.id } }">
            <button>Historique</button>
        </RouterLink></li>

        <li v-if="sgroup.synchronizedGroup && sgroup.right === 'admin'">
            <button @click="transform_SynchronizedGroup_into_group">Ne plus synchroniser ce groupe</button>
        </li>
        <li v-if="sgroup.group && isEmpty(sgroup.group.direct_members) && sgroup.right === 'admin'">
            <button @click="transform_group_into_SynchronizedGroup">Transformer en un groupe synchronisé</button>
        </li>
        <li v-if="sgroup.right === 'admin' && !modify_attrs.other && isEmpty(other_sgroup_attrs_having_values)">
            <button @click="start_modify_attr('other')">Champs supplémentaires</button>
        </li>

    </ul>

    <p><i>Mes droits sur ce {{sgroup.stem ? 'dossier' : 'groupe'}} : {{right2text[sgroup.right]}}</i></p>
</div>
</template>

<style scoped>
textarea {
    width: 100%;
    height: 10rem;
}
.modify_attr {
    display: inline-block;
}
.my-icon-check, .my-icon-close {
    --my-icon-size: 16px;
    vertical-align: middle;
}

thead > h5 {
    display: inline-block;
}
.float-right {
    float: right;
    margin-left: 1rem;
}

.label-and-val {
    display: flex;
    gap: 0.5rem;
    margin-bottom: 1rem;
}
.label-and-val > * {
    flex-basis: fill;
    flex-grow: 1;
}
.label-and-val > span.label {
    display: inline-block;
    max-width: 12rem;
}
.label-and-val > input {
    height: 1.3rem;
}

</style>