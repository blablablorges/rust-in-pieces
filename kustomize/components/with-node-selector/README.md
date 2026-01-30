# Use an external shipping service

If you want to use an external shipping service instead of the built-in shipping service provided by Online Boutique, you can use this Kustomize component.

## Use this component

From the `kustomize/` folder at the root level of this repository, execute this command:

```bash
kustomize edit add component components/with-shipping-external
```

## Configuration

To change the URL of the external shipping service, you must edit the two patch files in this component:
- `checkoutservice.yaml`: This file patches the `checkoutservice` Deployment to set the `SHIPPING_SERVICE_ADDR` environment variable to the URL of the external shipping service.
- `frontend.yaml`: This file patches the `frontend` Deployment to set the `SHIPPING_SERVICE_ADDR` environment variable to the URL of the external shipping service.
