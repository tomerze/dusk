# Campaign conflict

Kind `campaign_conflict`, severity `medium`. The twilight leader raises it the
first time one of a campaign's nodes is put in the state `conflict`, and writes
a `conflict` event to the campaign's history. The detail carries `message`,
`campaign_id` (the campaign whose rows are in conflict), and the `device_id` and
`installation_id` of that first node. One alert stays open per campaign. It
never resolves by itself.

## What it means

On one node only one `ensure_version` campaign per `version_key`, and only one
`ensure_config` campaign, is in effect: the one that started first among those
whose selector matches the node. This campaign's rows on nodes an earlier
campaign owns are in `conflict`; they count in no rate, nothing is sent to them,
and they wait again once nothing earlier claims the node
([overlapping campaigns](../campaigns.md#overlapping-campaigns)). Nothing is
wrong on the nodes; the two campaigns' intents overlap, and only the earlier one
acts there.

## How to confirm

The rows in conflict, and which running campaigns of the same kind share nodes
with this one:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>/nodes?state=conflict"
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>/overlap"
```

The node in the detail lists its campaign rows, so the earlier campaign that
owns it is there:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>"
```

## What to do

Decide which campaign should set the value on those nodes:

* The earlier one is right: leave it. This campaign acts on its other nodes, and
  its conflicted rows stay waiting.
* This one should win: complete the earlier campaign once you no longer need it
  enforcing its value. Its claim ends, and this campaign's rows wait again and
  are sent.

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "superseded by campaign <campaign_id>"}' \
      "$TWILIGHT/api/v1/campaigns/<earlier_campaign_id>/complete"
    ```

* Neither should run as it is: abort one and start it again with a selector
  that leaves the other's nodes out.

A campaign started with `allow_overlap: true` was meant to overlap; the alert
then only confirms it.

## How to resolve

Resolve it by hand once you have decided: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`.
